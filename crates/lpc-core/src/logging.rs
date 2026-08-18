//! A durable record of what the control plane did.
//!
//! Both 2026 incidents were expensive to diagnose for the same reason: nothing
//! on disk said when an account last refreshed, when the catalog last shrank,
//! or how many credential slots the keychain held at any point. The evidence
//! had to be reconstructed from registry snapshots after the fact.
//!
//! Every record is redacted *before* it reaches the file. Scrubbing a log after
//! writing it is not scrubbing it — the secret was already on disk, and on a
//! journalling filesystem it may stay there. Records use the local strength:
//! the file never leaves the machine it describes, and absolute paths are what
//! make it worth reading. `lpcctl doctor --share` is the path for anything that
//! does leave.

use crate::error::Result;
use crate::paths::AppPaths;
use crate::redact::{redact_with, RedactionLevel};
use chrono::Utc;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Beyond this the record is diagnostic noise, and a single runaway field can
/// otherwise fill the disk.
const MAX_RECORD_BYTES: usize = 16 * 1024;

/// Long enough to cover a weekend plus the week it takes to notice.
const MAX_RETAINED_LOG_DAYS: i64 = 14;

const FILE_PREFIX: &str = "lpc-";
const FILE_SUFFIX: &str = ".jsonl";

/// Point the global tracing subscriber at `<LPC_HOME>/logs/lpc-YYYYMMDD.jsonl`.
///
/// Safe to call from several processes and safe to call twice; a subscriber
/// that is already installed wins and this becomes a no-op.
pub fn init_file_logging(paths: &AppPaths) -> Result<()> {
    // The shim forwards the official CLI's stderr verbatim, and the desktop app
    // is built for the windows subsystem and has no stderr at all. Only a CLI
    // run by a human has somewhere useful to echo to, and only when it asks.
    init_logging(paths, std::env::var_os("LPC_LOG").is_some())
}

/// Install the file sink, optionally echoing the same redacted records to
/// stderr for an interactive run.
pub fn init_logging(paths: &AppPaths, echo_to_stderr: bool) -> Result<()> {
    let dir = paths.logs_dir();
    fs::create_dir_all(&dir)?;
    prune_expired_logs(&dir);

    let filter = EnvFilter::try_from_env("LPC_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_ansi(false)
        .with_target(true)
        .with_writer(DailyLog { dir });
    let stderr_layer = echo_to_stderr.then(|| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(RedactedStderr)
    });

    // Ignoring the error is deliberate: a command that refuses to run because
    // logging was already configured would be worse than one that quietly does
    // not log twice.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init();
    Ok(())
}

/// stderr is not a safe place for a credential either: it lands in scrollback,
/// in `script` captures, and in whatever CI collects. Same redaction as the
/// file, only the destination differs.
#[derive(Clone)]
struct RedactedStderr;

impl<'a> MakeWriter<'a> for RedactedStderr {
    type Writer = RedactedRecord;

    fn make_writer(&'a self) -> Self::Writer {
        RedactedRecord {
            sink: None,
            buffer: Vec::new(),
        }
    }
}

/// Opens the current day's file per record rather than holding a handle.
///
/// The cost is one open per event, which is irrelevant at control-plane event
/// rates, and it buys two things that a cached handle does not: several
/// processes (desktop, `lpcctl`, shim) can write to the same file, and a
/// process that stays up across midnight rolls over on its own.
#[derive(Clone)]
struct DailyLog {
    dir: PathBuf,
}

impl<'a> MakeWriter<'a> for DailyLog {
    type Writer = RedactedRecord;

    fn make_writer(&'a self) -> Self::Writer {
        RedactedRecord {
            sink: Some(self.dir.join(current_file_name())),
            buffer: Vec::new(),
        }
    }
}

/// Accumulates one formatted record, then redacts and writes it in a single
/// call so that concurrent writers interleave whole lines rather than halves.
/// `sink` is the day's log file, or stderr when there is none.
struct RedactedRecord {
    sink: Option<PathBuf>,
    buffer: Vec<u8>,
}

impl RedactedRecord {
    fn emit(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let record = String::from_utf8_lossy(&self.buffer).into_owned();
        self.buffer.clear();

        let mut safe = redact_with(RedactionLevel::Local, &record);
        if safe.len() > MAX_RECORD_BYTES {
            let mut cut = MAX_RECORD_BYTES;
            while cut > 0 && !safe.is_char_boundary(cut) {
                cut -= 1;
            }
            safe.truncate(cut);
            safe.push_str("…\n");
        }
        if !safe.ends_with('\n') {
            safe.push('\n');
        }

        let Some(path) = self.sink.as_ref() else {
            // lpc-allow-raw-write: stderr, not a file
            return io::stderr().write_all(safe.as_bytes());
        };
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?
            // Replacing the whole file, as the atomic helper does, would drop
            // every record already written to today's log.
            // lpc-allow-raw-write: append-only diagnostics, not control-plane state
            .write_all(safe.as_bytes())
    }
}

impl Write for RedactedRecord {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.emit()
    }
}

impl Drop for RedactedRecord {
    fn drop(&mut self) {
        // The fmt layer does not always flush explicitly. Losing a record to a
        // full disk is acceptable; panicking inside a logger is not.
        let _ = self.emit();
    }
}

fn current_file_name() -> String {
    format!("{FILE_PREFIX}{}{FILE_SUFFIX}", Utc::now().format("%Y%m%d"))
}

fn prune_expired_logs(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let cutoff = Utc::now().date_naive() - chrono::Duration::days(MAX_RETAINED_LOG_DAYS);
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stamp) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(FILE_PREFIX))
            .and_then(|name| name.strip_suffix(FILE_SUFFIX))
        else {
            continue;
        };
        let Ok(date) = chrono::NaiveDate::parse_from_str(stamp, "%Y%m%d") else {
            continue;
        };
        if date < cutoff {
            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(dir: &Path, text: &str) -> String {
        let mut writer = DailyLog {
            dir: dir.to_path_buf(),
        }
        .make_writer();
        writer.write_all(text.as_bytes()).unwrap();
        writer.flush().unwrap();
        fs::read_to_string(dir.join(current_file_name())).unwrap()
    }

    #[test]
    fn secrets_are_removed_before_the_record_reaches_the_disk() {
        let temp = tempfile::tempdir().unwrap();
        let written = record(
            temp.path(),
            r#"{"fields":{"message":"refresh failed","access_token":"u-should-not-persist"}}"#,
        );
        assert!(!written.contains("u-should-not-persist"));
        assert!(written.contains("refresh failed"));
    }

    #[test]
    fn records_stay_readable_and_line_delimited() {
        let temp = tempfile::tempdir().unwrap();
        record(temp.path(), r#"{"message":"first"}"#);
        let written = record(temp.path(), r#"{"message":"second"}"#);
        assert_eq!(written.lines().count(), 2);
        assert!(written.contains("first") && written.contains("second"));
    }

    #[test]
    fn a_runaway_record_cannot_fill_the_disk() {
        let temp = tempfile::tempdir().unwrap();
        let written = record(temp.path(), &"x".repeat(MAX_RECORD_BYTES * 4));
        assert!(written.len() < MAX_RECORD_BYTES * 2);
    }

    #[test]
    fn expired_files_are_pruned_and_current_ones_kept() {
        let temp = tempfile::tempdir().unwrap();
        let stale = temp.path().join("lpc-20200101.jsonl");
        let today = temp.path().join(current_file_name());
        let foreign = temp.path().join("keep-me.txt");
        for path in [&stale, &today, &foreign] {
            fs::write(path, b"x").unwrap();
        }

        prune_expired_logs(temp.path());

        assert!(!stale.exists(), "a log past the retention window survived");
        assert!(today.exists(), "today's log was pruned");
        assert!(foreign.exists(), "pruning removed an unrelated file");
    }
}
