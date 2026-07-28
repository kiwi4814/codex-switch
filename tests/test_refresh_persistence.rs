//! Regression tests for OAuth refresh-token rotation safety.
//!
//! OpenAI rotates `refresh_token` on every use and rejects replays with
//! `refresh_token_reused`. Any path that obtains a new token but fails to
//! persist it — or that replays a consumed token — permanently bricks the
//! profile, so these behaviours are covered end-to-end against a local mock.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};

/// Env vars are process-global; serialize every test that touches them.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct EnvVarGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: String) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[derive(Clone)]
struct Reply {
    status: StatusCode,
    body: Value,
}

fn reply(status: StatusCode, body: Value) -> Reply {
    Reply { status, body }
}

fn rotation(n: u32) -> Reply {
    reply(
        StatusCode::OK,
        json!({
            "id_token": format!("id_{n}"),
            "access_token": format!("access_{n}"),
            "refresh_token": format!("refresh_{n}"),
        }),
    )
}

#[derive(Default)]
struct MockState {
    /// Usage replies keyed by bearer token; the last entry repeats.
    usage: HashMap<String, Vec<Reply>>,
    usage_cursor: HashMap<String, usize>,
    /// Bearer tokens seen by the usage endpoint, in order.
    usage_calls: Vec<String>,
    /// Token-endpoint replies; the last entry repeats.
    token_replies: Vec<Reply>,
    /// `refresh_token` values seen by the token endpoint, in order.
    token_calls: Vec<String>,
}

type SharedState = Arc<Mutex<MockState>>;

struct MockServer {
    addr: SocketAddr,
    state: SharedState,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
}

impl MockServer {
    async fn start(usage: Vec<(String, Vec<Reply>)>, token_replies: Vec<Reply>) -> Self {
        let state: SharedState = Arc::new(Mutex::new(MockState {
            usage: usage.into_iter().collect(),
            token_replies,
            ..Default::default()
        }));

        let app = Router::new()
            .route("/backend-api/wham/usage", get(usage_handler))
            .route("/oauth/token", post(token_handler))
            .with_state(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        Self {
            addr,
            state,
            shutdown_tx,
        }
    }

    fn usage_url(&self) -> String {
        format!("http://{}/backend-api/wham/usage", self.addr)
    }

    fn token_url(&self) -> String {
        format!("http://{}/oauth/token", self.addr)
    }

    fn token_calls(&self) -> Vec<String> {
        self.state.lock().unwrap().token_calls.clone()
    }

    fn usage_calls(&self) -> Vec<String> {
        self.state.lock().unwrap().usage_calls.clone()
    }

    fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
    }
}

fn next_reply(replies: &[Reply], cursor: usize) -> Reply {
    replies[cursor.min(replies.len() - 1)].clone()
}

async fn usage_handler(State(state): State<SharedState>, headers: HeaderMap) -> impl IntoResponse {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();

    let mut guard = state.lock().unwrap();
    guard.usage_calls.push(bearer.clone());
    let Some(replies) = guard.usage.get(&bearer).cloned() else {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({"detail": format!("unknown token {bearer}")})),
        )
            .into_response();
    };
    let cursor = guard.usage_cursor.entry(bearer).or_insert(0);
    let chosen = next_reply(&replies, *cursor);
    *cursor += 1;
    (chosen.status, axum::Json(chosen.body)).into_response()
}

async fn token_handler(
    State(state): State<SharedState>,
    axum::Json(body): axum::Json<Value>,
) -> impl IntoResponse {
    let mut guard = state.lock().unwrap();
    let presented = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    guard.token_calls.push(presented);
    let cursor = guard.token_calls.len() - 1;
    let chosen = next_reply(&guard.token_replies, cursor);
    (chosen.status, axum::Json(chosen.body)).into_response()
}

fn expired_jwt() -> String {
    let exp = codex_switch::auth::now_unix_secs() - 3600;
    let payload = URL_SAFE_NO_PAD.encode(json!({"exp": exp}).to_string());
    format!("header.{payload}.signature")
}

fn write_profile(home: &Path, alias: &str, id: &str, access: &str, refresh: &str) -> PathBuf {
    let dir = home.join("profiles").join(alias);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("auth.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": id,
                "access_token": access,
                "refresh_token": refresh,
            },
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

fn stored_refresh_token(path: &Path) -> String {
    let raw = std::fs::read_to_string(path).unwrap();
    let val: Value = serde_json::from_str(&raw).unwrap();
    val.pointer("/tokens/refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Env + profile fixture. Field order matters: guards drop before `home`.
struct Fixture {
    profile_path: PathBuf,
    _guards: Vec<EnvVarGuard>,
    _home: tempfile::TempDir,
}

fn fixture(server: &MockServer, alias: &str, access_token: &str) -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let guards = vec![
        EnvVarGuard::set("CODEX_SWITCH_HOME", home.path().display().to_string()),
        EnvVarGuard::set(
            "CODEX_HOME",
            home.path().join("codex").display().to_string(),
        ),
        EnvVarGuard::set("CS_USAGE_URL", server.usage_url()),
        EnvVarGuard::set("CS_TOKEN_URL", server.token_url()),
        EnvVarGuard::remove("CS_RESET_CREDITS_URL"),
    ];
    let profile_path = write_profile(home.path(), alias, "old_id", access_token, "refresh_old");
    Fixture {
        profile_path,
        _guards: guards,
        _home: home,
    }
}

/// D1: a rotated refresh_token is single-use. If we obtain one and then drop it
/// because the follow-up usage call failed, the profile can never authenticate
/// again — so it must reach disk regardless of what happens afterwards.
#[tokio::test]
async fn rotated_refresh_token_is_persisted_even_when_usage_fails_afterwards() {
    let _lock = ENV_LOCK.lock().await;
    let server = MockServer::start(
        vec![
            (
                "old_access".to_string(),
                vec![reply(
                    StatusCode::UNAUTHORIZED,
                    json!({"detail": "access token expired"}),
                )],
            ),
            (
                "access_1".to_string(),
                vec![reply(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"detail": "upstream exploded"}),
                )],
            ),
        ],
        vec![rotation(1)],
    )
    .await;
    let fx = fixture(&server, "team1", "old_access");

    let err = codex_switch::usage::fetch_usage_retried_force("team1", &fx.profile_path, "team1")
        .await
        .expect_err("usage must fail in this scenario");

    assert_eq!(
        stored_refresh_token(&fx.profile_path),
        "refresh_1",
        "refresh token rotated by the auth server was lost; profile is now bricked (error was: {err})"
    );
    assert_eq!(
        server.token_calls(),
        vec!["refresh_old".to_string()],
        "the consumed refresh token must never be replayed"
    );
    server.shutdown();
}

/// D2: once a refresh succeeds the old refresh_token is dead server-side.
/// Later retry rounds must present the rotated token, otherwise a transient
/// usage failure escalates into a permanent `refresh_token_reused` lockout.
#[tokio::test]
async fn each_retry_round_presents_the_rotated_refresh_token() {
    let _lock = ENV_LOCK.lock().await;
    let server = MockServer::start(
        vec![
            (
                "old_access".to_string(),
                vec![reply(
                    StatusCode::UNAUTHORIZED,
                    json!({"detail": "expired"}),
                )],
            ),
            (
                "access_1".to_string(),
                vec![reply(
                    StatusCode::UNAUTHORIZED,
                    json!({"detail": "expired"}),
                )],
            ),
            (
                "access_2".to_string(),
                vec![reply(
                    StatusCode::UNAUTHORIZED,
                    json!({"detail": "expired"}),
                )],
            ),
            (
                "access_3".to_string(),
                vec![reply(
                    StatusCode::UNAUTHORIZED,
                    json!({"detail": "expired"}),
                )],
            ),
        ],
        vec![rotation(1), rotation(2), rotation(3)],
    )
    .await;
    let fx = fixture(&server, "team2", "old_access");

    let _ =
        codex_switch::usage::fetch_usage_retried_force("team2", &fx.profile_path, "team2").await;

    assert_eq!(
        server.token_calls(),
        vec![
            "refresh_old".to_string(),
            "refresh_1".to_string(),
            "refresh_2".to_string(),
        ],
        "retries replayed an already-consumed refresh token"
    );
    assert_eq!(
        server.usage_calls(),
        vec![
            "old_access".to_string(),
            "access_1".to_string(),
            "access_1".to_string(),
            "access_2".to_string(),
            "access_2".to_string(),
            "access_3".to_string(),
        ],
        "retries must carry the refreshed access token"
    );
    server.shutdown();
}

/// D3: OpenAI returns `error` as an object, not the OAuth-standard string.
/// The actionable server message must survive deserialization instead of being
/// replaced by a serde type error.
#[tokio::test]
async fn object_shaped_oauth_error_is_reported_with_code_and_message() {
    let _lock = ENV_LOCK.lock().await;
    let server = MockServer::start(
        vec![(
            "old_access".to_string(),
            vec![reply(StatusCode::UNAUTHORIZED, json!({"detail": "expired"}))],
        )],
        vec![reply(
            StatusCode::UNAUTHORIZED,
            json!({
                "error": {
                    "code": "refresh_token_reused",
                    "message": "Your refresh token has already been used to generate a new access token. Please try signing in again.",
                    "param": null,
                    "type": "invalid_request_error",
                }
            }),
        )],
    )
    .await;
    let fx = fixture(&server, "team3", "old_access");

    let err = codex_switch::usage::fetch_usage_retried_force("team3", &fx.profile_path, "team3")
        .await
        .expect_err("a rejected refresh token must fail");

    assert!(
        err.detail.contains("refresh_token_reused"),
        "server error code missing from user-facing detail: {}",
        err.detail
    );
    assert!(
        err.detail.contains("Please try signing in again."),
        "server error message missing from user-facing detail: {}",
        err.detail
    );
    server.shutdown();
}

/// D3/D4: the OAuth-standard string shape must keep working too.
#[tokio::test]
async fn string_shaped_oauth_error_is_reported_with_description() {
    let _lock = ENV_LOCK.lock().await;
    let server = MockServer::start(
        vec![(
            "old_access".to_string(),
            vec![reply(
                StatusCode::UNAUTHORIZED,
                json!({"detail": "expired"}),
            )],
        )],
        vec![reply(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "invalid_grant",
                "error_description": "The refresh token is invalid or has expired.",
            }),
        )],
    )
    .await;
    let fx = fixture(&server, "team5", "old_access");

    let err = codex_switch::usage::fetch_usage_retried_force("team5", &fx.profile_path, "team5")
        .await
        .expect_err("an invalid_grant refresh must fail");

    assert!(
        err.detail.contains("invalid_grant"),
        "server error code missing from user-facing detail: {}",
        err.detail
    );
    assert!(
        err.detail
            .contains("The refresh token is invalid or has expired."),
        "server error description missing from user-facing detail: {}",
        err.detail
    );
    server.shutdown();
}

/// D4: `refresh_token_reused` is terminal. Retrying burns wall-clock time on a
/// slow proxy and cannot succeed, so the auth endpoint must be hit exactly once
/// — including no second attempt inside the same round after a failed
/// proactive refresh.
#[tokio::test]
async fn reused_refresh_token_stops_retrying_after_a_single_auth_request() {
    let _lock = ENV_LOCK.lock().await;
    let stale_access = expired_jwt();
    let server = MockServer::start(
        vec![(
            stale_access.clone(),
            vec![reply(StatusCode::UNAUTHORIZED, json!({"detail": "expired"}))],
        )],
        vec![reply(
            StatusCode::UNAUTHORIZED,
            json!({
                "error": {
                    "code": "refresh_token_reused",
                    "message": "Your refresh token has already been used to generate a new access token. Please try signing in again.",
                    "param": null,
                    "type": "invalid_request_error",
                }
            }),
        )],
    )
    .await;
    let fx = fixture(&server, "team4", &stale_access);

    let err = codex_switch::usage::fetch_usage_retried_force("team4", &fx.profile_path, "team4")
        .await
        .expect_err("a reused refresh token must fail");

    assert_eq!(
        server.token_calls().len(),
        1,
        "terminal auth failure must not be retried, saw {:?} (error: {})",
        server.token_calls(),
        err.detail
    );
    assert!(
        err.summary.contains("refresh_token_reused"),
        "short summary must name the terminal auth failure: {}",
        err.summary
    );
    server.shutdown();
}
