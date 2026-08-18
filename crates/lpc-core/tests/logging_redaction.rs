//! Proves redaction survives the real tracing pipeline, not just a unit call.
//!
//! The unit tests in `logging.rs` drive the writer directly. That checks the
//! redactor but skips everything tracing does in between: field formatting,
//! JSON encoding, escaping. A secret only has to survive one of those steps to
//! reach the disk, so the whole chain is exercised here.
//!
//! One test per binary on purpose — installing a global subscriber is a
//! process-wide, one-shot operation, and a second test in this file would
//! silently run against the first one's subscriber.

use lpc_core::{logging::init_logging, AppPaths};

/// Shaped like a credential and unique enough that finding it anywhere in the
/// log is unambiguous.
const PLANTED: &str = "lpcfake-must-not-persist-4c1d8a";

#[test]
fn a_secret_logged_as_a_field_never_lands_on_disk() {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(temp.path());
    init_logging(&paths, false).unwrap();

    tracing::info!(
        credential = format!("access_token={PLANTED}"),
        note = "probe",
        "logging redaction probe"
    );

    let logs = temp.path().join("logs");
    let written: String = std::fs::read_dir(&logs)
        .expect("logs directory")
        .flatten()
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
        .collect();

    assert!(
        !written.is_empty(),
        "nothing was written to {}, so this test proved nothing",
        logs.display()
    );
    assert!(
        written.contains("logging redaction probe"),
        "the probe record never reached the log, so the assertion below is vacuous"
    );
    assert!(
        !written.contains(PLANTED),
        "a credential-shaped field value reached the log file verbatim"
    );
    assert!(
        written.contains("[REDACTED]"),
        "the field survived unredacted rather than being replaced"
    );
}
