//! Makes a bridged message send answerable after its caller gave up waiting.
//!
//! The host runs a bridged command to completion even when the client that asked
//! for it is killed (a caller's timeout), so the message goes out but its result is
//! lost, and the caller's retry sends it a second time. The host therefore keeps the
//! outcome of each send: a retry of the same send from the same caller waits for
//! the one still running, or gets back the result that never reached its caller.
//! A send whose result was delivered is never replayed: sending the same text again
//! afterwards is a new message.
// Only the Windows host serves the bridge; elsewhere the journal is test-only.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::host_bridge::HostBridgeResponse;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Long enough for any caller to retry, short enough that a later identical message
/// is not mistaken for a retry.
const UNDELIVERED_RETENTION: Duration = Duration::from_secs(15 * 60);

/// Shortcuts that post a message. Their retry is the case that duplicates.
const SEND_COMMANDS: &[(&str, &str)] = &[("im", "+messages-send"), ("im", "+messages-reply")];

/// Shells and wrappers that a caller starts afresh for every attempt; the caller is
/// the first ancestor above them.
#[cfg(windows)]
const WRAPPERS: &[&str] = &[
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "bash.exe",
    "sh.exe",
    "conhost.exe",
    "lark-cli.exe",
];

/// Whether forwarded arguments post a message (not its help or dry run).
pub(crate) fn is_message_send(args: &[String]) -> bool {
    let mut words = args.iter().map(String::as_str);
    let mut command = Vec::with_capacity(2);
    while let Some(word) = words.next() {
        if word == "--lpc-account" {
            words.next();
            continue;
        }
        command.push(word);
        if command.len() == 2 {
            break;
        }
    }
    let [first, second] = command[..] else {
        return false;
    };
    SEND_COMMANDS.contains(&(first, second))
        && !args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--help" | "-h" | "--dry-run"))
}

/// Identifies the long-lived process behind a send, stable across its retries:
/// the nearest ancestor that is not a per-attempt shell, with its start time so a
/// reused PID is a different caller. `None` means retries cannot be matched.
#[cfg(windows)]
pub(crate) fn caller_identity() -> Option<String> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        ProcessRefreshKind::new().with_exe(UpdateKind::Always),
    );
    let mut child_started_at = u64::MAX;
    let mut next = system.process(Pid::from_u32(std::process::id()))?.parent();
    for _ in 0..16 {
        let process = system.process(next?)?;
        // A parent younger than its child is a reused PID, not the real parent.
        if process.start_time() > child_started_at {
            return None;
        }
        child_started_at = process.start_time();
        let name = process.name().to_string_lossy().to_ascii_lowercase();
        if !WRAPPERS.contains(&name.as_str()) {
            return Some(format!("{}:{}", process.pid(), process.start_time()));
        }
        next = process.parent();
    }
    None
}

#[cfg(not(windows))]
pub(crate) fn caller_identity() -> Option<String> {
    None
}

/// Everything that makes two requests the same send.
pub(crate) fn send_key(
    caller: &str,
    route: &str,
    current_dir: &std::path::Path,
    args: &[String],
    stdin: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    for part in [caller, route, &current_dir.to_string_lossy()] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    for arg in args {
        hasher.update(arg.as_bytes());
        hasher.update([0]);
    }
    hasher.update(stdin.unwrap_or_default().as_bytes());
    hex::encode(hasher.finalize())
}

/// The message ID in a send's output, for the log. Nothing else of it is logged.
pub(crate) fn message_id(stdout: &str) -> Option<&str> {
    let start = stdout.find("om_")?;
    let rest = &stdout[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

enum Entry {
    Running {
        waiters: usize,
    },
    Finished {
        response: HostBridgeResponse,
        /// Retries that were waiting when the send ended; each takes the result once.
        waiters: usize,
        /// Set while the send's own caller has not received the result.
        undelivered_since: Option<Instant>,
    },
}

pub(crate) enum Begin {
    /// Nothing to reuse: run the command, then call [`SendJournal::finish`].
    Run,
    /// The outcome of the same send, which its own caller never received.
    Replay(HostBridgeResponse),
}

#[derive(Default)]
pub(crate) struct SendJournal {
    entries: Mutex<HashMap<String, Entry>>,
    changed: Condvar,
}

impl SendJournal {
    pub(crate) fn begin(&self, key: &str) -> Begin {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries.retain(|_, entry| match entry {
            Entry::Running { .. } => true,
            Entry::Finished {
                waiters,
                undelivered_since,
                ..
            } => {
                *waiters > 0
                    || undelivered_since
                        .is_some_and(|since| since.elapsed() < UNDELIVERED_RETENTION)
            }
        });
        let mut waiting = false;
        loop {
            match entries.get_mut(key) {
                None => {
                    entries.insert(key.to_owned(), Entry::Running { waiters: 0 });
                    return Begin::Run;
                }
                Some(Entry::Running { waiters }) => {
                    if !waiting {
                        *waiters += 1;
                        waiting = true;
                    }
                }
                Some(Entry::Finished {
                    response,
                    waiters,
                    undelivered_since,
                }) => {
                    if waiting || undelivered_since.is_some() {
                        // A retry got the answer, so the caller has it now.
                        if waiting {
                            *waiters -= 1;
                        }
                        *undelivered_since = None;
                    } else {
                        // Delivered, with earlier retries still collecting it. This
                        // request arrived after the send ended, so it is a new send.
                        entries = self
                            .changed
                            .wait(entries)
                            .unwrap_or_else(|error| error.into_inner());
                        continue;
                    }
                    let response = replayed(response);
                    if *waiters == 0 && undelivered_since.is_none() {
                        entries.remove(key);
                        self.changed.notify_all();
                    }
                    return Begin::Replay(response);
                }
            }
            entries = self
                .changed
                .wait(entries)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    /// Records how a send that [`Begin::Run`] started ended. Retries that were
    /// already waiting get its result whether or not its own caller received it.
    pub(crate) fn finish(&self, key: &str, response: &HostBridgeResponse, delivered: bool) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let waiters = match entries.get(key) {
            Some(Entry::Running { waiters }) => *waiters,
            _ => 0,
        };
        if waiters == 0 && delivered {
            entries.remove(key);
        } else {
            entries.insert(
                key.to_owned(),
                Entry::Finished {
                    response: copy(response),
                    waiters,
                    undelivered_since: (!delivered).then(Instant::now),
                },
            );
        }
        self.changed.notify_all();
    }
}

fn copy(response: &HostBridgeResponse) -> HostBridgeResponse {
    HostBridgeResponse {
        exit_code: response.exit_code,
        stdout: response.stdout.clone(),
        stderr: response.stderr.clone(),
    }
}

fn replayed(response: &HostBridgeResponse) -> HostBridgeResponse {
    let mut response = copy(response);
    response.stderr.push_str(
        "[LPC_SEND_REPLAYED] this send already ran once for this caller; returning its result instead of sending again\n",
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    fn sent(id: &str) -> HostBridgeResponse {
        HostBridgeResponse {
            exit_code: 0,
            stdout: format!("{{\"message_id\":\"{id}\"}}\n"),
            stderr: String::new(),
        }
    }

    #[test]
    fn only_real_sends_are_tracked() {
        assert!(is_message_send(&args(&[
            "im",
            "+messages-send",
            "--text",
            "hi"
        ])));
        assert!(is_message_send(&args(&[
            "--lpc-account",
            "宇进",
            "im",
            "+messages-reply",
            "--message-id",
            "om_1"
        ])));
        assert!(!is_message_send(&args(&["im", "+messages-send", "--help"])));
        assert!(!is_message_send(&args(&[
            "im",
            "+messages-send",
            "--dry-run",
            "--text",
            "hi"
        ])));
        assert!(!is_message_send(&args(&["im", "+chat-messages-list"])));
        assert!(!is_message_send(&args(&["auth", "status"])));
        assert!(!is_message_send(&args(&["--lpc-account", "im"])));
    }

    #[test]
    fn key_separates_callers_routes_and_content() {
        let base = send_key("1:1", "a", "C:\\w".as_ref(), &args(&["im", "x"]), None);
        assert_eq!(
            base,
            send_key("1:1", "a", "C:\\w".as_ref(), &args(&["im", "x"]), None)
        );
        assert_ne!(
            base,
            send_key("2:1", "a", "C:\\w".as_ref(), &args(&["im", "x"]), None)
        );
        assert_ne!(
            base,
            send_key("1:1", "b", "C:\\w".as_ref(), &args(&["im", "x"]), None)
        );
        assert_ne!(
            base,
            send_key("1:1", "a", "C:\\w".as_ref(), &args(&["im", "y"]), None)
        );
        assert_ne!(
            base,
            send_key("1:1", "a", "C:\\w".as_ref(), &args(&["im", "x"]), Some("-"))
        );
    }

    #[test]
    fn message_id_is_found_in_json_output() {
        assert_eq!(
            message_id("{\n  \"message_id\": \"om_x100b6\",\n}"),
            Some("om_x100b6")
        );
        assert_eq!(message_id("error"), None);
    }

    #[test]
    fn an_undelivered_result_is_replayed_once() {
        let journal = SendJournal::default();
        assert!(matches!(journal.begin("k"), Begin::Run));
        journal.finish("k", &sent("om_1"), false);
        let Begin::Replay(response) = journal.begin("k") else {
            panic!("the retry must not send again");
        };
        assert!(response.stdout.contains("om_1"));
        assert!(response.stderr.contains("LPC_SEND_REPLAYED"));
        assert!(matches!(journal.begin("k"), Begin::Run));
    }

    #[test]
    fn a_delivered_result_is_not_replayed_to_a_later_send() {
        let journal = SendJournal::default();
        assert!(matches!(journal.begin("k"), Begin::Run));
        journal.finish("k", &sent("om_1"), true);
        assert!(matches!(journal.begin("k"), Begin::Run));
    }

    #[test]
    fn a_retry_waits_for_the_running_send_and_shares_its_result() {
        let journal = Arc::new(SendJournal::default());
        assert!(matches!(journal.begin("k"), Begin::Run));
        let retry = {
            let journal = journal.clone();
            std::thread::spawn(move || journal.begin("k"))
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(!retry.is_finished(), "the retry must wait, not send");
        journal.finish("k", &sent("om_1"), true);
        let Begin::Replay(response) = retry.join().unwrap() else {
            panic!("the retry must get the running send's result");
        };
        assert!(response.stdout.contains("om_1"));
    }

    #[test]
    fn different_sends_do_not_wait_for_each_other() {
        let journal = SendJournal::default();
        assert!(matches!(journal.begin("a"), Begin::Run));
        assert!(matches!(journal.begin("b"), Begin::Run));
    }

    #[cfg(windows)]
    #[test]
    fn the_caller_is_stable_and_is_not_a_wrapper_shell() {
        let first = caller_identity().expect("the test runner has a parent");
        assert_eq!(Some(first.clone()), caller_identity());
        let pid: u32 = first.split(':').next().unwrap().parse().unwrap();
        assert_ne!(pid, std::process::id());
    }
}
