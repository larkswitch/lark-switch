//! Runs real client processes through the real bridge against a fake official
//! CLI in a private data root: concurrent reads beside a send, and a send whose
//! caller is killed mid-flight. It never touches a real profile or credential.
#![cfg(windows)]

use lpc_core::{start_host_bridge, AppPaths};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const RECEIVER: &str = "ou_test_receiver";

fn fake_cli() -> Option<PathBuf> {
    // target/<profile>/deps/<test>.exe -> target/<profile>/examples/fake_lark_cli.exe
    let exe = std::env::current_exe().unwrap();
    let fake = exe.parent()?.parent()?.join("examples/fake_lark_cli.exe");
    if fake.exists() {
        return Some(fake);
    }
    assert!(
        std::env::var_os("LPC_REQUIRE_DEPLOY_ARTIFACT").is_none(),
        "build the fake CLI with `cargo build -p lpc-core --examples`"
    );
    eprintln!("skip: fake CLI not built");
    None
}

struct Bridge {
    _temp: tempfile::TempDir,
    root: PathBuf,
    fake: PathBuf,
}

impl Bridge {
    fn start(fake: PathBuf, send_ms: u64) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("lpc");
        let paths = AppPaths::new(&root);
        paths.ensure_layout().unwrap();
        std::fs::copy(&fake, paths.bin_dir().join("lark-cli.exe")).unwrap();
        // The fake CLI runs in the caller's directory, the root, and reads this.
        std::fs::write(root.join("send-ms"), send_ms.to_string()).unwrap();
        // Windows stops this test's process tree at this size.
        std::env::set_var("LPC_HOST_PROCESS_LIMIT", "64");
        start_host_bridge(paths).unwrap();
        Self {
            _temp: temp,
            root,
            fake,
        }
    }

    fn command(&self, args: &[&str]) -> Child {
        Command::new(&self.fake)
            .args(args)
            .env("FAKE_ROLE", "client")
            .env("LPC_HOME", &self.root)
            .env_remove("LPC_HOST_EXECUTOR_PID")
            .env_remove("LPC_HOST_BRIDGE_HOPS")
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn sends(&self) -> usize {
        std::fs::read_to_string(self.root.join("sent.log"))
            .map(|log| log.lines().count())
            .unwrap_or(0)
    }
}

fn send_args(text: &str) -> Vec<String> {
    [
        "im",
        "+messages-send",
        "--as",
        "user",
        "--user-id",
        RECEIVER,
        "--text",
        text,
    ]
    .map(str::to_owned)
    .to_vec()
}

fn message_id(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let start = stdout
        .find("om_")
        .unwrap_or_else(|| panic!("no message ID in {stdout:?}, stderr {:?}", stderr(output)));
    stdout[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn wait_until(what: &str, deadline: Duration, done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn args_ref(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

#[test]
fn reads_run_together_and_never_wait_for_a_send() {
    let Some(fake) = fake_cli() else { return };
    let bridge = Bridge::start(fake, 4000);
    let send = bridge.command(&args_ref(&send_args("side by side")));
    let reads: Vec<_> = (0..5)
        .map(|_| bridge.command(&["im", "+chat-messages-list", "--chat-id", "oc_x"]))
        .collect();

    for read in reads {
        let output = read.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok");
    }
    assert_eq!(
        bridge.sends(),
        0,
        "every read finished while the send still ran"
    );
    let output = send.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    message_id(&output);
    assert_eq!(bridge.sends(), 1);

    // Each line is one read's start and end; all five must have been running at once.
    let spans: Vec<(u128, u128)> = std::fs::read_to_string(bridge.root.join("reads.log"))
        .unwrap()
        .lines()
        .map(|line| {
            let (start, end) = line.split_once(' ').unwrap();
            (start.parse().unwrap(), end.parse().unwrap())
        })
        .collect();
    assert_eq!(spans.len(), 5);
    let last_start = spans.iter().map(|span| span.0).max().unwrap();
    let first_end = spans.iter().map(|span| span.1).min().unwrap();
    assert!(
        last_start < first_end,
        "reads ran one after another: {spans:?}"
    );
}

#[test]
fn a_retry_after_the_caller_was_killed_gets_the_same_message_instead_of_sending_again() {
    let Some(fake) = fake_cli() else { return };
    let bridge = Bridge::start(fake, 1500);
    let args = send_args("killed mid-flight");

    let mut first = bridge.command(&args_ref(&args));
    std::thread::sleep(Duration::from_millis(500));
    first.kill().unwrap();
    first.wait().unwrap();
    // The host still finishes the send its caller gave up on.
    wait_until("the abandoned send", Duration::from_secs(10), || {
        bridge.sends() == 1
    });

    let retry = bridge.command(&args_ref(&args)).wait_with_output().unwrap();
    assert_eq!(retry.status.code(), Some(0), "{}", stderr(&retry));
    assert!(
        stderr(&retry).contains("LPC_SEND_REPLAYED"),
        "{}",
        stderr(&retry)
    );
    message_id(&retry);
    assert_eq!(
        bridge.sends(),
        1,
        "the retry must not send a second message"
    );
}

#[test]
fn a_retry_while_the_killed_send_still_runs_waits_for_it() {
    let Some(fake) = fake_cli() else { return };
    let bridge = Bridge::start(fake, 2000);
    let args = send_args("retried while running");

    let mut first = bridge.command(&args_ref(&args));
    std::thread::sleep(Duration::from_millis(500));
    first.kill().unwrap();
    first.wait().unwrap();
    let retry = bridge.command(&args_ref(&args)).wait_with_output().unwrap();

    assert_eq!(retry.status.code(), Some(0), "{}", stderr(&retry));
    assert!(
        stderr(&retry).contains("LPC_SEND_REPLAYED"),
        "{}",
        stderr(&retry)
    );
    message_id(&retry);
    assert_eq!(
        bridge.sends(),
        1,
        "the retry must not send a second message"
    );
}

#[test]
fn a_send_whose_result_arrived_is_not_replayed_to_the_next_identical_send() {
    let Some(fake) = fake_cli() else { return };
    let bridge = Bridge::start(fake, 200);
    let args = send_args("sent twice on purpose");

    let first = bridge.command(&args_ref(&args)).wait_with_output().unwrap();
    let second = bridge.command(&args_ref(&args)).wait_with_output().unwrap();

    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    assert_eq!(second.status.code(), Some(0), "{}", stderr(&second));
    assert!(!stderr(&second).contains("LPC_SEND_REPLAYED"));
    assert_ne!(message_id(&first), message_id(&second));
    assert_eq!(bridge.sends(), 2);
}
