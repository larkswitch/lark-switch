//! Durability contract: control-plane state is never written in place.
//!
//! A partially written `catalog.json` loses every account and App (the
//! 2026-07-17 profile-loss incident); a partially written account
//! `config.json` costs that account its authorization. `lpc_core::atomic`
//! is the single sanctioned way to put bytes on disk: temp file, fsync,
//! atomic replace.
//!
//! Writes that land inside a scratch directory and are published later by a
//! single rename are legitimate. They opt out with a
//! `// lpc-allow-raw-write: <reason>` marker on the offending line or the line
//! above it, which keeps the reason next to the code instead of in an allowlist
//! that nobody re-reads.

mod common;

/// Crates whose sources persist control-plane state.
const SCANNED_SOURCE_DIRS: &[&str] = &[
    "crates/lpc-core/src",
    "crates/lpcctl/src",
    "crates/lpc-shim/src",
    "apps/desktop/src-tauri/src",
];

/// The one module allowed to implement raw file replacement.
const ATOMIC_IMPLEMENTATION: &str = "crates/lpc-core/src/atomic.rs";

const ALLOW_MARKER: &str = "lpc-allow-raw-write:";

/// Raw write primitives, paired with the sanctioned replacement.
const FORBIDDEN_WRITES: &[(&str, &str)] = &[
    ("fs::write(", "atomic::write_bytes_atomic"),
    ("File::create(", "atomic::write_bytes_atomic"),
    ("fs::copy(", "fs::read + atomic::write_bytes_atomic"),
    ("fs::rename(", "atomic::write_bytes_atomic"),
    ("serde_json::to_writer", "atomic::write_json_atomic"),
    (".write_all(", "atomic::write_bytes_atomic"),
];

#[test]
fn state_is_written_through_the_atomic_helper() {
    let root = common::repo_root();
    let mut violations = Vec::new();

    for directory in SCANNED_SOURCE_DIRS {
        for path in common::collect_files(&root.join(directory), "rs") {
            let relative = common::relative(&path, &root);
            if relative == ATOMIC_IMPLEMENTATION {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read Rust source");
            collect_violations(&relative, &source, &mut violations);
        }
    }

    assert!(
        violations.is_empty(),
        "raw file writes outside {ATOMIC_IMPLEMENTATION}:\n{}\n\n\
         Route the write through the listed helper, or — if it targets a scratch \
         path that is published by a later atomic rename — annotate the line with \
         `// {ALLOW_MARKER} <reason>`.",
        violations.join("\n")
    );
}

/// The contract above is only worth anything while the helper still does the
/// work. Guard its three load-bearing steps.
#[test]
fn atomic_helper_still_writes_through_a_temp_file() {
    let root = common::repo_root();
    let path = root.join(ATOMIC_IMPLEMENTATION);
    let source = std::fs::read_to_string(&path).expect("read atomic.rs");

    for expected in ["create_new(true)", "sync_all()", "atomic_replace("] {
        assert!(
            source.contains(expected),
            "{ATOMIC_IMPLEMENTATION} no longer contains `{expected}`; \
             the durability guarantee other modules rely on has been weakened."
        );
    }
}

fn collect_violations(relative: &str, source: &str, violations: &mut Vec<String>) {
    let lines: Vec<&str> = common::production_region(source).lines().collect();
    for (offset, line) in lines.iter().enumerate() {
        let allowed_above = offset
            .checked_sub(1)
            .is_some_and(|previous| lines[previous].contains(ALLOW_MARKER));
        if line.contains(ALLOW_MARKER) || allowed_above {
            continue;
        }
        for (pattern, replacement) in FORBIDDEN_WRITES {
            if !line.contains(pattern) {
                continue;
            }
            // Writing to a child process' stdin is not a durability concern.
            if *pattern == ".write_all(" && line.contains("stdin") {
                continue;
            }
            violations.push(format!(
                "  {relative}:{} uses `{pattern}` — use {replacement}",
                offset + 1
            ));
        }
    }
}
