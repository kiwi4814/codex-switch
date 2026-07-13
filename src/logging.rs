use anyhow::{Context, Result};
use chrono::{Days, Local, NaiveDate};
use fs4::FileExt;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tracing_subscriber::fmt::MakeWriter;

const LOG_PREFIX: &str = "codex-switch";
const MAX_LOG_AGE_DAYS: u64 = 3;
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct FileLogWriter {
    state: Arc<Mutex<LogState>>,
}

struct LogState {
    dir: PathBuf,
}

pub(crate) fn file_log_writer() -> Result<FileLogWriter> {
    let dir = crate::auth::app_home()?.join("logs");
    create_private_log_dir(&dir)
        .with_context(|| format!("creating log directory {}", dir.display()))?;
    Ok(FileLogWriter {
        state: Arc::new(Mutex::new(LogState { dir })),
    })
}

fn create_private_log_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

#[cfg(unix)]
fn tighten_file_permissions(file: &fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(fs::Permissions::from_mode(0o600))
}

impl<'a> MakeWriter<'a> for FileLogWriter {
    type Writer = LogFile;

    fn make_writer(&'a self) -> Self::Writer {
        LogFile {
            state: Arc::clone(&self.state),
        }
    }
}

pub(crate) struct LogFile {
    state: Arc<Mutex<LogState>>,
}

impl Write for LogFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let retained = if buf.len() as u64 > MAX_LOG_BYTES {
            &buf[buf.len() - MAX_LOG_BYTES as usize..]
        } else {
            buf
        };
        let state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("log writer lock poisoned"))?;
        append_log(&state.dir, Local::now().date_naive(), retained)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn append_log(dir: &Path, today: NaiveDate, bytes: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    let mut lock_options = OpenOptions::new();
    lock_options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        lock_options.mode(0o600);
    }
    let lock = lock_options.open(dir.join(".lock"))?;
    #[cfg(unix)]
    tighten_file_permissions(&lock)?;
    FileExt::lock(&lock)?;
    let result = (|| {
        prune_log_files(dir, today)?;
        enforce_log_size_limit(dir, today, bytes.len() as u64)?;
        let mut log_options = OpenOptions::new();
        log_options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            log_options.mode(0o600);
        }
        let mut file = log_options.open(log_path(dir, today))?;
        #[cfg(unix)]
        tighten_file_permissions(&file)?;
        file.write_all(bytes)
    })();
    FileExt::unlock(&lock)?;
    result
}

fn prune_log_files(dir: &Path, today: NaiveDate) -> io::Result<()> {
    let oldest = today - Days::new(MAX_LOG_AGE_DAYS - 1);
    for (path, date, _) in log_files(dir)? {
        if date < oldest {
            fs::remove_file(path)?;
        }
    }
    enforce_log_size_limit(dir, today, 0)
}

fn enforce_log_size_limit(dir: &Path, today: NaiveDate, incoming: u64) -> io::Result<()> {
    let current = log_path(dir, today);
    let mut files = log_files(dir)?;
    files.sort_by_key(|(_, date, _)| *date);
    let mut total = files.iter().map(|(_, _, size)| *size).sum::<u64>();

    for (path, _, size) in &files {
        if total.saturating_add(incoming) <= MAX_LOG_BYTES {
            return Ok(());
        }
        if *path != current {
            fs::remove_file(path)?;
            total = total.saturating_sub(*size);
        }
    }

    if total.saturating_add(incoming) > MAX_LOG_BYTES && current.exists() {
        fs::OpenOptions::new()
            .write(true)
            .open(&current)?
            .set_len(0)?;
    }
    Ok(())
}

fn log_files(dir: &Path) -> io::Result<Vec<(PathBuf, NaiveDate, u64)>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(date) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(log_date)
        else {
            continue;
        };
        if entry.file_type()?.is_file() {
            files.push((path, date, entry.metadata()?.len()));
        }
    }
    Ok(files)
}

fn log_path(dir: &Path, date: NaiveDate) -> PathBuf {
    dir.join(format!("{LOG_PREFIX}.{date}.log"))
}

fn log_date(filename: &str) -> Option<NaiveDate> {
    filename
        .strip_prefix(&format!("{LOG_PREFIX}."))?
        .strip_suffix(".log")
        .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_log(dir: &Path, day: NaiveDate, bytes: u64) {
        let file = fs::File::create(log_path(dir, day)).unwrap();
        file.set_len(bytes).unwrap();
    }

    #[test]
    fn retains_only_the_latest_three_calendar_days() {
        let dir = tempfile::tempdir().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 7, 12).unwrap();
        for day in 8..=12 {
            create_log(
                dir.path(),
                NaiveDate::from_ymd_opt(2026, 7, day).unwrap(),
                1,
            );
        }

        prune_log_files(dir.path(), today).unwrap();

        assert!(!log_path(dir.path(), NaiveDate::from_ymd_opt(2026, 7, 8).unwrap()).exists());
        assert!(!log_path(dir.path(), NaiveDate::from_ymd_opt(2026, 7, 9).unwrap()).exists());
        assert!(log_path(dir.path(), NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()).exists());
        assert!(log_path(dir.path(), today).exists());
    }

    #[test]
    fn removes_oldest_logs_to_keep_total_at_ten_mebibytes() {
        let dir = tempfile::tempdir().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 7, 12).unwrap();
        for day in 10..=12 {
            create_log(
                dir.path(),
                NaiveDate::from_ymd_opt(2026, 7, day).unwrap(),
                5 * 1024 * 1024,
            );
        }

        prune_log_files(dir.path(), today).unwrap();

        assert!(!log_path(dir.path(), NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()).exists());
        assert!(log_path(dir.path(), NaiveDate::from_ymd_opt(2026, 7, 11).unwrap()).exists());
        assert!(log_path(dir.path(), today).exists());
    }

    #[test]
    fn appending_never_exceeds_ten_mebibytes() {
        let dir = tempfile::tempdir().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 7, 12).unwrap();
        create_log(dir.path(), today, MAX_LOG_BYTES);

        append_log(dir.path(), today, b"next event").unwrap();

        assert!(fs::metadata(log_path(dir.path(), today)).unwrap().len() <= MAX_LOG_BYTES);
    }

    #[cfg(unix)]
    #[test]
    fn append_log_tightens_directory_lock_and_log_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 7, 12).unwrap();
        let lock_path = dir.path().join(".lock");
        let current_log = log_path(dir.path(), today);
        fs::File::create(&lock_path).unwrap();
        fs::File::create(&current_log).unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o666)).unwrap();
        fs::set_permissions(&current_log, fs::Permissions::from_mode(0o666)).unwrap();

        append_log(dir.path(), today, b"private event").unwrap();

        assert_eq!(
            fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(lock_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(current_log).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
