//! `pluryprint update …`
//!
//! `check` is the CLI face of [`crate::ipc::update::check_for_updates`]: the
//! same pure [`crate::update::evaluate`] over the same release manifest of the
//! configured channel (FR-085). `channel` reads/writes `updater.channel` through the settings core.
//!
//! `install` is the CLI face of [`crate::ipc::update::install_update`]: the
//! same [`crate::update::install`] core (manifest, signature verification,
//! per-user install, FR-087's major-version rule), driven through a windowless
//! Tauri app because the updater needs an `AppHandle`. Confirmation is the one
//! CLI-only concern: `--yes` skips it, an interactive terminal is asked, and a
//! non-interactive run without `--yes` is refused rather than guessed.
//!
//! Two Windows-only rules follow from how the installer works there. The
//! per-user NSIS setup runs in passive mode and closes a running Pluryprint
//! without asking, so `install` refuses while the app is open (it may be
//! printing) — [`refuse_install_while_app_runs`]. And the updater plugin ends
//! the process itself (`std::process::exit`) once the installer is launched,
//! so the result is printed from the hand-off hook ([`Handoff`]) instead of
//! after `install` returns, and the installer is told not to relaunch the CLI's
//! own command line.
//!
//! On Linux the windowless app still opens a GTK connection to the display,
//! and a `.deb` update asks for the administrator password in a dialog, so
//! `install` needs a graphical session: without `DISPLAY` or
//! `WAYLAND_DISPLAY` (SSH, a text console) it refuses at once with a clear
//! message ([`refuse_install_without_display`]) instead of failing inside GTK.

use crate::cli::args::UpdateCmd;
use crate::cli::exit::{CliError, ExitCode};
use crate::cli::output::Emitter;
use crate::cli::Paths;
use crate::ipc::settings::{get_app_settings_at, set_app_settings_at};
use crate::util::error::IpcError;

/// Run an `update` subcommand.
pub fn run(paths: &Paths, emitter: Emitter, cmd: UpdateCmd) -> Result<(), CliError> {
    match cmd {
        UpdateCmd::Check => {
            let channel = crate::settings::Config::load_from(&paths.config)
                .updater
                .channel;
            let release = crate::update::fetch_configured_latest(channel)
                .map_err(|e| CliError::from(IpcError::from(e)))?;
            let check = crate::update::evaluate(
                env!("CARGO_PKG_VERSION"),
                release,
                crate::queue::job::now_rfc3339(),
            );
            emitter.emit(&check, || match (&check.available, &check.latest) {
                (true, Some(latest)) => format!(
                    "Update available: {latest} (running {})\n{}",
                    check.current,
                    check.url.clone().unwrap_or_default()
                ),
                _ => format!("Up to date ({})", check.current),
            });
            Ok(())
        }

        UpdateCmd::Install { yes } => install(paths, emitter, yes),

        UpdateCmd::Channel { channel } => {
            let mut settings = get_app_settings_at(&paths.config)?;
            match channel {
                None => {
                    let current = settings.updater.channel;
                    emitter.emit(&serde_json::json!({ "channel": current }), || {
                        format!("Update channel: {}", channel_name(current))
                    });
                }
                Some(new) => {
                    settings.updater.channel = new.to_settings();
                    let saved = set_app_settings_at(&paths.config, &paths.profiles, settings)?;
                    let channel = saved.updater.channel;
                    emitter.emit(&serde_json::json!({ "channel": channel }), || {
                        format!("Update channel set to {}", channel_name(channel))
                    });
                }
            }
            Ok(())
        }
    }
}

/// How `update install` obtains the user's go-ahead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    /// `--yes`: proceed, majors included (the flag is the explicit opt-in).
    Granted,
    /// Ask on the terminal.
    Prompt,
    /// No `--yes` and nobody to ask: refuse instead of guessing.
    Refused,
}

/// Decide how to confirm an install. Pure.
pub fn confirmation(yes: bool, interactive: bool) -> Confirmation {
    match (yes, interactive) {
        (true, _) => Confirmation::Granted,
        (false, true) => Confirmation::Prompt,
        (false, false) => Confirmation::Refused,
    }
}

/// The question asked before installing; a major jump says so (FR-087).
pub fn prompt_text(current: &str, version: &str, major: bool) -> String {
    if major {
        format!(
            "Pluryprint {version} is a MAJOR update from {current} and may change how the app \
             works. Install it now? [y/N] "
        )
    } else {
        format!("Install Pluryprint {version} (running {current}) now? [y/N] ")
    }
}

/// Whether a typed answer is a yes. Pure.
pub fn is_yes(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes" | "s" | "si" | "sí"
    )
}

/// The result of an install that is handing over to the installer, reported
/// exactly once — from the updater's before-exit hook on Windows (where the
/// process never returns from `install`), or after `install` returns elsewhere.
#[derive(Debug, Default)]
pub struct Handoff {
    version: std::sync::OnceLock<String>,
    reported: std::sync::atomic::AtomicBool,
}

impl Handoff {
    /// Record the version being installed, once it is known.
    pub fn set_version(&self, version: &str) {
        let _ = self.version.set(version.to_string());
    }

    /// Report the result through `emit` unless that already happened. Nothing
    /// is reported before the version is known.
    pub fn report(&self, emit: impl FnOnce(&crate::update::install::UpdateInstallResult)) {
        use std::sync::atomic::Ordering;
        let Some(version) = self.version.get() else {
            return;
        };
        if self.reported.swap(true, Ordering::SeqCst) {
            return;
        }
        emit(&crate::update::install::UpdateInstallResult {
            installed: Some(version.clone()),
            // Windows: the installer finishes the job and nothing is relaunched
            // for a command-line run; there is no restart for the user to do.
            restart_required: !cfg!(windows),
        });
    }
}

/// Refuse `update install` while the app is open, on Windows: the passive
/// per-user installer closes a running Pluryprint without asking, which
/// would cut off whatever it is printing. Pure; `lock` is what the instance
/// lock says ([`crate::cli::instance::state`]).
///
/// A lock that cannot be checked is refused too, as `queue cancel` does: on
/// a folder without file locking the app runs unlocked, and "could not tell"
/// must not turn into "the app is closed" right before the installer closes
/// it.
pub fn refuse_install_while_app_runs(
    windows: bool,
    lock: &crate::cli::instance::LockState,
) -> Result<(), CliError> {
    use crate::cli::instance::LockState;
    if !windows {
        return Ok(());
    }
    match lock {
        LockState::Free => Ok(()),
        LockState::Held => Err(IpcError::InvalidState(
            "Pluryprint is open, and installing an update would close it in the middle of \
             whatever it is printing — nothing was installed. Update from the app (About → \
             Install update), or close it and run this again"
                .to_string(),
        )
        .into()),
        LockState::Unknown(why) => Err(IpcError::InvalidState(format!(
            "could not check whether Pluryprint is open ({why}), and installing an update \
             would close it in the middle of whatever it is printing — nothing was installed. \
             Update from the app (About → Install update), or close it and run this again"
        ))
        .into()),
    }
}

/// Refuse `update install` on Linux when there is no graphical session to
/// open the updater in. Pure; `display` and `wayland` are the values of
/// `DISPLAY` and `WAYLAND_DISPLAY` (an empty one counts as unset).
pub fn refuse_install_without_display(
    linux: bool,
    display: Option<&std::ffi::OsStr>,
    wayland: Option<&std::ffi::OsStr>,
) -> Result<(), CliError> {
    let set = |v: Option<&std::ffi::OsStr>| v.is_some_and(|v| !v.is_empty());
    if !linux || set(display) || set(wayland) {
        return Ok(());
    }
    Err(IpcError::UpdateInstallFailed(
        "`update install` needs a graphical session on Linux (neither DISPLAY nor \
         WAYLAND_DISPLAY is set): the updater opens a hidden window, and a .deb update asks \
         for the administrator password in a dialog — nothing was installed. Run it from a \
         terminal on the desktop, or download the release from the releases page"
            .to_string(),
    )
    .into())
}

fn print_result(emitter: Emitter, result: &crate::update::install::UpdateInstallResult) {
    emitter.emit(result, || {
        match (&result.installed, result.restart_required) {
            (Some(v), true) => format!("Installed Pluryprint {v}; restart the app to use it"),
            (Some(v), false) => format!("Installing Pluryprint {v}"),
            (None, _) => "Nothing to install".to_string(),
        }
    });
}

/// `update install [--yes]`.
fn install(paths: &Paths, emitter: Emitter, yes: bool) -> Result<(), CliError> {
    use std::io::{IsTerminal, Write};
    use std::sync::Arc;

    use crate::update::install as core;

    refuse_install_while_app_runs(
        cfg!(windows),
        &crate::cli::instance::state(&crate::cli::instance::lock_path_for(&paths.queue)),
    )?;
    refuse_install_without_display(
        cfg!(target_os = "linux"),
        std::env::var_os("DISPLAY").as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
    )?;

    let app = crate::headless_updater_app().map_err(|e| {
        CliError::from(IpcError::UpdateInstallFailed(format!(
            "could not start the updater: {e}"
        )))
    })?;
    let handle = app.handle().clone();

    let handoff = Arc::new(Handoff::default());
    let hook = Arc::clone(&handoff);
    let channel = crate::settings::Config::load_from(&paths.config)
        .updater
        .channel;
    let Some(update) = tauri::async_runtime::block_on(core::find_update_with(
        &handle,
        channel,
        core::Relaunch::Nothing,
        move || hook.report(|result| print_result(emitter, result)),
    ))?
    else {
        let none = core::UpdateInstallResult {
            installed: None,
            restart_required: false,
        };
        emitter.emit(&none, || {
            format!(
                "Up to date ({}); nothing to install",
                env!("CARGO_PKG_VERSION")
            )
        });
        return Ok(());
    };

    let major = core::is_major_bump(&update.current_version, &update.version);
    match confirmation(yes, std::io::stdin().is_terminal()) {
        Confirmation::Granted => {}
        Confirmation::Refused => {
            return Err(CliError::from(IpcError::InvalidRequest(format!(
                "Pluryprint {} is available; re-run with --yes to install it without a prompt",
                update.version
            ))));
        }
        Confirmation::Prompt => {
            eprint!(
                "{}",
                prompt_text(&update.current_version, &update.version, major)
            );
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            let _ = std::io::stdin().read_line(&mut answer);
            if !is_yes(&answer) {
                return Err(CliError::new(
                    ExitCode::Cancelled,
                    IpcError::InvalidState("update not installed: cancelled".to_string()),
                ));
            }
        }
    }

    if !emitter.json {
        eprintln!(
            "Downloading Pluryprint {} and verifying its signature…",
            update.version
        );
    }
    // Reaching this point *is* the explicit opt-in FR-087 asks for: `--yes`
    // (blanket, so no version is pinned) or a typed "yes" to a prompt that
    // named this very version.
    let confirmed = (!yes).then_some(update.version.as_str());
    handoff.set_version(&update.version);
    let result =
        tauri::async_runtime::block_on(core::install(&update, true, confirmed, |_, _| {}))?;
    // Only reached where the updater returns (not Windows); the hook has not
    // reported anything then.
    handoff.report(|_| print_result(emitter, &result));
    Ok(())
}

/// The wire spelling of a channel, matching `config.toml` and the bindings.
fn channel_name(channel: crate::settings::UpdateChannel) -> &'static str {
    use crate::settings::UpdateChannel;
    match channel {
        UpdateChannel::Stable => "stable",
        UpdateChannel::Beta => "beta",
        UpdateChannel::Nightly => "nightly",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::CliChannel;
    use crate::settings::{Config, UpdateChannel};

    fn paths_in(dir: &std::path::Path) -> Paths {
        Paths {
            config: dir.join("config.toml"),
            profiles: dir.join("profiles.toml"),
            queue: dir.join("queue.json"),
        }
    }

    #[test]
    fn fr_008_channel_round_trips_through_config() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        Config::default().save_to(&paths.config).unwrap();

        run(
            &paths,
            Emitter::new(true),
            UpdateCmd::Channel {
                channel: Some(CliChannel::Beta),
            },
        )
        .unwrap();

        assert_eq!(
            Config::load_from(&paths.config).updater.channel,
            UpdateChannel::Beta
        );
    }

    #[test]
    fn fr_087_install_needs_an_explicit_yes() {
        assert_eq!(confirmation(true, false), Confirmation::Granted);
        assert_eq!(confirmation(true, true), Confirmation::Granted);
        assert_eq!(confirmation(false, true), Confirmation::Prompt);
        // A script without --yes is refused, never silently allowed.
        assert_eq!(confirmation(false, false), Confirmation::Refused);
    }

    #[test]
    fn fr_087_the_prompt_calls_out_a_major_update() {
        assert!(prompt_text("1.4.0", "2.0.0", true).contains("MAJOR"));
        assert!(!prompt_text("1.4.0", "1.5.0", false).contains("MAJOR"));
        assert!(prompt_text("1.4.0", "1.5.0", false).ends_with("[y/N] "));
    }

    #[test]
    fn fr_087_only_an_explicit_answer_is_a_yes() {
        for yes in ["y", "Y", "yes", " YES \n", "s", "sí"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n", "no", "maybe", "yess"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }

    #[test]
    fn fr_009_install_failures_use_the_documented_code() {
        let signature: CliError = IpcError::UpdateSignatureInvalid("tampered".into()).into();
        assert_eq!(signature.exit, ExitCode::UpdateInstallFailed);
        let network: CliError = IpcError::UpdateInstallFailed("offline".into()).into();
        assert_eq!(network.exit, ExitCode::UpdateInstallFailed);
    }

    /// FR-009: on Windows the installer closes a running app without asking,
    /// so a scripted `update install` refuses while the app is open instead of
    /// cutting off a print (an `InvalidState` error, exit 1).
    #[test]
    fn fr_009_install_refuses_while_the_app_runs_on_windows() {
        use crate::cli::instance::LockState;
        let err = refuse_install_while_app_runs(true, &LockState::Held).unwrap_err();
        assert_eq!(err.exit, ExitCode::Generic);
        assert!(err.error.to_string().contains("nothing was installed"));
        assert!(refuse_install_while_app_runs(true, &LockState::Free).is_ok());
        // macOS/Linux replace the bundle without closing the app.
        assert!(refuse_install_while_app_runs(false, &LockState::Held).is_ok());
    }

    /// FR-009 / NFR-010: a lock that cannot be checked (a profile folder
    /// without file locking) is not evidence the app is closed, so the passive
    /// installer is not started over a print that may be running.
    #[test]
    fn fr_009_install_refuses_when_the_app_lock_cannot_be_checked() {
        use crate::cli::instance::LockState;
        let unknown = LockState::Unknown("locking not supported".into());
        let err = refuse_install_while_app_runs(true, &unknown).unwrap_err();
        assert!(matches!(err.error, IpcError::InvalidState(_)));
        let text = err.error.to_string();
        assert!(text.contains("could not check"), "{text}");
        assert!(text.contains("nothing was installed"), "{text}");
        assert!(refuse_install_while_app_runs(false, &unknown).is_ok());
    }

    /// Linux: without a display the updater cannot start; `update install`
    /// says so up front (exit 11, nothing installed) instead of failing inside
    /// GTK. Any display server will do, and other OSes are never refused.
    #[test]
    fn fr_009_install_on_linux_needs_a_graphical_session() {
        use std::ffi::OsStr;
        let err = refuse_install_without_display(true, None, None).unwrap_err();
        assert_eq!(err.exit, ExitCode::UpdateInstallFailed);
        let text = err.error.to_string();
        assert!(
            text.contains("DISPLAY") && text.contains("nothing was installed"),
            "{text}"
        );
        assert!(refuse_install_without_display(true, Some(OsStr::new("")), None).is_err());
        assert!(refuse_install_without_display(true, Some(OsStr::new(":0")), None).is_ok());
        assert!(refuse_install_without_display(true, None, Some(OsStr::new("wayland-0"))).is_ok());
        assert!(refuse_install_without_display(false, None, None).is_ok());
    }

    /// FR-010: the result is reported exactly once, whether from the
    /// updater's before-exit hook (Windows, where the process then exits) or
    /// after `install` returns — and never before the version is known.
    #[test]
    fn fr_010_the_install_result_is_reported_once_from_the_handoff() {
        let handoff = Handoff::default();
        let mut seen = Vec::new();
        handoff.report(|r| seen.push(r.clone()));
        assert!(seen.is_empty(), "nothing before the version is known");

        handoff.set_version("1.5.0");
        handoff.report(|r| seen.push(r.clone()));
        handoff.report(|r| seen.push(r.clone()));
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].installed.as_deref(), Some("1.5.0"));
        assert_eq!(seen[0].restart_required, !cfg!(windows));
    }
}
