//! Stand-in for both ends of the host bridge in `tests/bridge_send_outcome.rs`.
//!
//! With `FAKE_ROLE=client` it is an agent-shell command that forwards its
//! arguments through the bridge. Otherwise it is the official CLI the host runs:
//! a send waits, records itself in `FAKE_SENT_LOG` and prints a message ID; any
//! other command waits briefly and prints `ok`.
use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if std::env::var("FAKE_ROLE").as_deref() == Ok("client") {
        let root = std::env::var_os("LPC_HOME").expect("LPC_HOME");
        let paths = lpc_core::AppPaths::new(root);
        let forwarded: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
        match lpc_core::execute_via_host_bridge(&paths, &forwarded) {
            Ok(response) => {
                print!("{}", response.stdout);
                eprint!("{}", response.stderr);
                std::process::exit(response.exit_code);
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(70);
            }
        }
    }

    if args.iter().any(|arg| arg == "+messages-send") {
        let send_ms = std::fs::read_to_string("send-ms")
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(2000);
        std::thread::sleep(Duration::from_millis(send_ms));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("sent.log")
            .unwrap();
        writeln!(file, "{}", args.join(" ")).unwrap();
        let id = format!("om_fake{}", std::process::id());
        println!(r#"{{"ok":true,"data":{{"message_id":"{id}"}}}}"#);
    } else {
        let started = now_ms();
        std::thread::sleep(Duration::from_millis(1500));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("reads.log")
            .unwrap();
        writeln!(file, "{started} {}", now_ms()).unwrap();
        println!("ok");
    }
}
