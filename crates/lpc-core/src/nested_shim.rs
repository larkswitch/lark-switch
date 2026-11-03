//! Lets a shim started underneath another shim reuse that shim's execution.
//!
//! The official CLI spawns children that call `lark-cli` again: `apps +init`
//! runs `git`, and `git` runs `lark-cli apps git-credential-helper`. The outer
//! shim holds the keychain lock until those children exit, so the child already
//! has that protection. It must also stay on the outer shim's route: a tray switch
//! between the two calls must not hand the child a different identity.
//!
//! The environment variable only names a candidate. It is inherited by anything
//! the CLI spawns and can be set by anyone, so authority comes from the real
//! process tree: the named process must be an ancestor running this same shim.

use std::path::{Path, PathBuf};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// Set by a shim on the official CLI it launches while holding the keychain lock.
pub const SHIM_OWNER_PID_ENV: &str = "LPC_SHIM_OWNER_PID";

const CONFIG_DIR_ENV: &str = "LARKSUITE_CLI_CONFIG_DIR";
const MAX_ANCESTORS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessLink {
    pid: u32,
    exe: Option<PathBuf>,
}

/// Returns the official CLI of the enclosing shim when the current process is
/// verifiably nested under one, or `None` for a normal top-level invocation.
pub fn nested_official_cli() -> Option<PathBuf> {
    let owner_pid = std::env::var(SHIM_OWNER_PID_ENV)
        .ok()?
        .parse::<u32>()
        .ok()?;
    // The account route travels in this variable; without it there is nothing to reuse.
    std::env::var_os(CONFIG_DIR_ENV).filter(|value| !value.is_empty())?;
    let shim = std::env::current_exe().ok()?.canonicalize().ok()?;
    official_below_owner(&ancestor_chain(), owner_pid, &shim)
}

/// Walks from the current process upward. A parent started after its child means
/// the parent PID was reused by an unrelated process, so the walk stops there.
fn ancestor_chain() -> Vec<ProcessLink> {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        ProcessRefreshKind::new().with_exe(UpdateKind::Always),
    );
    let mut chain = Vec::new();
    let mut next = Some(Pid::from_u32(std::process::id()));
    let mut child_started_at = u64::MAX;
    while let Some(pid) = next {
        let Some(process) = system.process(pid) else {
            break;
        };
        if chain.len() == MAX_ANCESTORS || process.start_time() > child_started_at {
            break;
        }
        child_started_at = process.start_time();
        chain.push(ProcessLink {
            pid: pid.as_u32(),
            exe: process.exe().and_then(|path| path.canonicalize().ok()),
        });
        next = process.parent();
    }
    chain
}

/// `chain[0]` is the current process. The process directly below the owner is the
/// official CLI that the owner launched, so its path never comes from the environment.
fn official_below_owner(chain: &[ProcessLink], owner_pid: u32, shim: &Path) -> Option<PathBuf> {
    let owner_index = chain
        .iter()
        .skip(1)
        .position(|link| link.pid == owner_pid)?
        + 1;
    if chain[owner_index].exe.as_deref() != Some(shim) {
        return None;
    }
    chain[owner_index - 1]
        .exe
        .clone()
        .filter(|official| official != shim)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIM: &str = r"C:\lpc\bin\lark-cli.exe";
    const OFFICIAL: &str = r"C:\lpc\runtime\lark-cli.exe";

    fn link(pid: u32, exe: &str) -> ProcessLink {
        ProcessLink {
            pid,
            exe: Some(PathBuf::from(exe)),
        }
    }

    fn git_helper_chain() -> Vec<ProcessLink> {
        vec![
            link(1, SHIM),
            link(2, r"C:\git\sh.exe"),
            link(3, r"C:\git\git.exe"),
            link(4, OFFICIAL),
            link(5, SHIM),
            link(6, r"C:\host\desktop.exe"),
        ]
    }

    #[test]
    fn git_credential_helper_under_a_shim_reuses_its_official_cli() {
        assert_eq!(
            official_below_owner(&git_helper_chain(), 5, Path::new(SHIM)),
            Some(PathBuf::from(OFFICIAL))
        );
    }

    #[test]
    fn inherited_pid_of_a_process_outside_the_tree_is_not_authority() {
        assert_eq!(
            official_below_owner(&git_helper_chain(), 99, Path::new(SHIM)),
            None
        );
    }

    #[test]
    fn ancestor_running_a_different_program_is_not_authority() {
        assert_eq!(
            official_below_owner(&git_helper_chain(), 3, Path::new(SHIM)),
            None
        );
    }

    #[test]
    fn current_process_cannot_name_itself() {
        assert_eq!(
            official_below_owner(&git_helper_chain(), 1, Path::new(SHIM)),
            None
        );
    }

    #[test]
    fn shim_directly_under_a_shim_never_launches_itself() {
        let chain = vec![link(1, SHIM), link(5, SHIM)];
        assert_eq!(official_below_owner(&chain, 5, Path::new(SHIM)), None);
    }

    #[test]
    fn unreadable_owner_path_is_not_authority() {
        let mut chain = git_helper_chain();
        chain[4].exe = None;
        assert_eq!(official_below_owner(&chain, 5, Path::new(SHIM)), None);
    }
}
