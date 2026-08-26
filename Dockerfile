# syntax=docker/dockerfile:1

# ---- build stage -----------------------------------------------------------
# reqwest is configured for rustls only (Cargo.toml), so no OpenSSL headers or
# pkg-config are needed here beyond what the rust image already ships.
FROM rust:1.98-bookworm AS builder

WORKDIR /src
COPY . .

# Cache mounts keep the registry and target directory out of the image layer, so
# an interrupted or repeated build reuses the dependency compilation instead of
# starting over. The binary is copied out inside the same RUN because /src/target
# does not exist once the mount is released.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --locked --release \
    && cp target/release/codex-switch /out-codex-switch

# ---- runtime stage ---------------------------------------------------------
# ca-certificates: outbound HTTPS to the ChatGPT/Codex endpoints.
# tzdata:          daemon.five_hour_warmup_times is matched against
#                  chrono::Local::now() (src/daemon/loop_runner.rs:75), so the
#                  container needs a real zoneinfo database plus TZ.
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        tzdata \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /out-codex-switch /usr/local/bin/codex-switch

# Both homes are resolved from these variables (src/auth.rs codex_home_from_values
# and app_home); HOME is only the fallback, but it is set so no code path has to
# resolve a home directory for an arbitrary --user UID that is absent from
# /etc/passwd.
ENV HOME=/data \
    CODEX_HOME=/data/codex \
    CODEX_SWITCH_HOME=/data/codex-switch

# Mount points for the host's ~/.codex and ~/.codex-switch. Both are always
# bind-mounted at runtime; these directories only keep the paths valid.
RUN mkdir -p /data/codex /data/codex-switch

WORKDIR /data

ENTRYPOINT ["codex-switch"]
# Plain `daemon start` detaches a child with null stdio (src/daemon/mod.rs
# start_detached) and the container would exit immediately, so the long-running
# form must be --foreground.
CMD ["daemon", "start", "--foreground"]
