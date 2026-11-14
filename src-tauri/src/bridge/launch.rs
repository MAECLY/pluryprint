//! Starting the app with a committed job's handoff, detached from the bridge.
//!
//! The browser ends the host process as soon as the extension closes the
//! port, and on macOS and Linux it signals the host's whole process group.
//! The app must outlive that, so it is started in a process group of its own
//! (a new session's worth of separation on Unix: `process_group(0)`; on
//! Windows `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`, plus
//! `CREATE_BREAKAWAY_FROM_JOB` when the browser's job object allows it), with
//! no standard handles inherited — stdout is the protocol channel.
//!
//! The command line is the context-menu one every integration uses:
//! `pluryprint --invoke=context-menu --files-from <handoff> --action <a>
//! [--profile <id>]`. A running instance receives it through the
//! single-instance plugin (a named pipe on Windows, a Unix socket on macOS),
//! so the selection lands in the open window instead of a second one.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::cli::args::ContextAction;

/// The app executable's file name next to the bridge.
#[cfg(windows)]
pub const GUI_EXE: &str = "pluryprint.exe";
/// The app executable's file name next to the bridge.
#[cfg(not(windows))]
pub const GUI_EXE: &str = "pluryprint";

/// The bridge executable's file name.
#[cfg(windows)]
pub const BRIDGE_EXE: &str = "pluryprint-bridge.exe";
/// The bridge executable's file name.
#[cfg(not(windows))]
pub const BRIDGE_EXE: &str = "pluryprint-bridge";

/// The app executable that sits next to `bridge` (same folder: the install
/// folder on Windows, `Pluryprint.app/Contents/MacOS` on macOS, `/usr/bin`
/// for the .deb).
pub fn gui_next_to(bridge: &Path) -> PathBuf {
    bridge.with_file_name(GUI_EXE)
}

/// The arguments the app is started with.
pub fn gui_args(handoff: &Path, action: ContextAction, profile: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "--invoke=context-menu".to_string(),
        "--files-from".to_string(),
        handoff.to_string_lossy().into_owned(),
        "--action".to_string(),
        action.to_string(),
    ];
    if let Some(profile) = profile {
        args.push("--profile".to_string());
        args.push(profile.to_string());
    }
    args
}

/// Start `exe` with `args`, detached, and return once it has started.
pub fn spawn_detached(exe: &Path, args: &[String]) -> std::io::Result<()> {
    let mut command = Command::new(exe);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The app resolves nothing against its working directory, but the
    // bridge's (the browser sets it to the bridge's folder) must not be held
    // open by a long-lived child either.
    if let Some(dir) = exe.parent() {
        command.current_dir(dir);
    }
    detach_and_spawn(command)
}

#[cfg(unix)]
fn detach_and_spawn(mut command: Command) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    command.spawn().map(|_| ())
}

#[cfg(windows)]
fn detach_and_spawn(mut command: Command) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    // Chrome may run the host inside a job object that is closed (and its
    // processes killed) with the port. Breaking away keeps the app alive; a
    // job that forbids breakaway makes CreateProcess fail with access
    // denied, and then the app is started inside it as a fallback.
    command.creation_flags(base | CREATE_BREAKAWAY_FROM_JOB);
    match command.spawn() {
        Ok(_) => Ok(()),
        Err(e) if e.raw_os_error() == Some(5) => {
            command.creation_flags(base);
            command.spawn().map(|_| ())
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr_125_the_app_is_started_with_the_context_menu_command_line() {
        let args = gui_args(
            Path::new("/data/handoff/pluryfact-1.json"),
            ContextAction::Configure,
            None,
        );
        assert_eq!(
            args,
            [
                "--invoke=context-menu",
                "--files-from",
                "/data/handoff/pluryfact-1.json",
                "--action",
                "configure"
            ]
        );
        let args = gui_args(
            Path::new("/h.json"),
            ContextAction::PrintDefault,
            Some("builtin-duplex-bw"),
        );
        assert_eq!(
            &args[3..],
            [
                "--action",
                "print-default",
                "--profile",
                "builtin-duplex-bw"
            ]
        );
        // And the app's own parser reads it back as a context-menu launch.
        let mut argv = vec!["pluryprint".to_string()];
        argv.extend(args);
        assert_eq!(
            crate::cli::classify(&argv[1..]),
            crate::cli::Intent::ContextMenu
        );
        let cli = crate::cli::args::Cli::parse_context_menu(&argv).unwrap();
        assert_eq!(cli.action, ContextAction::PrintDefault);
        assert_eq!(cli.profile.as_deref(), Some("builtin-duplex-bw"));
    }

    #[test]
    fn fr_125_the_app_is_the_bridges_sibling() {
        let bridge = Path::new("/Applications/Pluryprint.app/Contents/MacOS").join(BRIDGE_EXE);
        assert_eq!(
            gui_next_to(&bridge),
            Path::new("/Applications/Pluryprint.app/Contents/MacOS").join(GUI_EXE)
        );
    }

    /// The child is started and the call returns at once, without waiting
    /// for it (a long-lived window must not hold the bridge).
    #[cfg(unix)]
    #[test]
    fn fr_125_spawn_returns_without_waiting() {
        let started = std::time::Instant::now();
        spawn_detached(Path::new("/bin/sleep"), &["5".to_string()]).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(4));
        assert!(spawn_detached(Path::new("/no/such/pluryprint"), &[]).is_err());
    }
}
