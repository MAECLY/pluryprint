//! Fake programs for tests: `/bin/sh` scripts written to a temporary folder
//! and started in place of a real tool (Unix only).
//!
//! A script cannot be started while any process holds it open for writing
//! (`ETXTBSY`). The test runner runs tests on many threads, and one that
//! forks while another is still writing its script hands that open
//! descriptor to the child, which holds it until it execs (close-on-exec
//! only closes it then). Writing to another name and renaming the file into
//! place does not help: the descriptor names the file, not the path. So
//! [`script`] starts the script once as a probe (`PLURYPRINT_FAKE_PROBE`, on
//! which it exits before doing anything) and retries on `ETXTBSY` until it
//! starts. Once it has, nothing can hold it open for writing any more: only
//! a fork made while this thread had it open could, and that fork has since
//! exec'd or exited.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The variable on which every fake exits at once ([`script`]).
const PROBE: &str = "PLURYPRINT_FAKE_PROBE";

/// Write `body` as an executable `/bin/sh` script at `dir/name` and return
/// its path once it can be started (see the module documentation).
pub(crate) fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!("#!/bin/sh\n[ -n \"${PROBE}\" ] && exit 0\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..250 {
        match std::process::Command::new(&path).env(PROBE, "1").status() {
            Ok(status) if status.success() => return path,
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                std::thread::sleep(Duration::from_millis(20))
            }
            other => panic!("{} does not start: {other:?}", path.display()),
        }
    }
    panic!("{} stayed busy", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake starts at once after [`script`] returns, runs its body, and
    /// the probe run left nothing behind.
    #[test]
    fn a_fake_starts_and_its_probe_leaves_no_trace() {
        let dir = tempfile::tempdir().unwrap();
        let mark = dir.path().join("ran");
        let fake = script(dir.path(), "tool", &format!(": > '{}'", mark.display()));
        assert!(!mark.exists(), "the probe ran the body");
        assert!(std::process::Command::new(&fake)
            .status()
            .unwrap()
            .success());
        assert!(mark.exists());
    }

    /// Many fakes written and started from many threads at once, the shape
    /// that used to fail now and then with `ETXTBSY` ("Text file busy").
    #[test]
    fn fakes_written_while_other_threads_fork_always_start() {
        let dir = tempfile::tempdir().unwrap();
        std::thread::scope(|s| {
            for t in 0..8 {
                let dir = dir.path();
                s.spawn(move || {
                    for n in 0..25 {
                        let fake = script(dir, &format!("t{t}-{n}"), "exit 0");
                        let status = std::process::Command::new(&fake).status();
                        assert!(status.is_ok_and(|s| s.success()), "t{t}-{n}");
                    }
                });
            }
        });
    }
}
