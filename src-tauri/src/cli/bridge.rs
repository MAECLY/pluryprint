//! `pluryprint bridge …` — register the browser bridge (FR-127, ADR-0016).
//!
//! The Windows installer and, on macOS, the app itself at every start already
//! register the bridge, so this is for everything else: a Linux install, a
//! repair, a development build of Pluryfact whose extension id has to be
//! allowed (`--extra-origin`), or a script that wants to know the state
//! (`bridge status --json`). The exit codes are the existing ones: 0, 1 for a
//! failure to write, 2 for a bad origin.

use crate::bridge::register::{self, BrowserStatus, Target};
use crate::cli::args::{BridgeCmd, CliBrowser};
use crate::cli::exit::{CliError, ExitCode};
use crate::cli::output::{table, Emitter};
use crate::util::error::IpcError;

/// Run a `bridge` subcommand.
pub fn run(emitter: Emitter, cmd: BridgeCmd) -> Result<(), CliError> {
    let target = Target::current().ok_or_else(|| {
        CliError::new(
            ExitCode::Generic,
            IpcError::Internal("cannot resolve the home folder or this program's path".into()),
        )
    })?;
    let statuses = run_at(&target, cmd)?;
    emitter.emit(&statuses, || render(&statuses));
    Ok(())
}

/// [`run`] against an injected [`Target`], returning the statuses to print.
pub fn run_at(target: &Target, cmd: BridgeCmd) -> Result<Vec<BrowserStatus>, CliError> {
    match cmd {
        BridgeCmd::Status => Ok(register::Browser::ALL
            .iter()
            .map(|b| target.status(*b))
            .collect()),
        BridgeCmd::Install {
            browser,
            extra_origin,
        } => {
            if let Some(bad) = extra_origin
                .iter()
                .find(|o| !register::is_extension_origin(o))
            {
                return Err(CliError::usage(format!(
                    "'{bad}' is not an extension origin (chrome-extension://<32-letter id>/)"
                )));
            }
            if register::is_translocated(&target.bridge) {
                return Err(failed(
                    "Pluryprint is running from a temporary location; move it to Applications first",
                ));
            }
            if !target.bridge.is_file() {
                return Err(failed(format!(
                    "the bridge is not installed next to this program ({})",
                    target.bridge.display()
                )));
            }
            // `all` registers with the browsers this user has; naming one
            // registers with it whether or not it was found.
            let browsers: Vec<_> = browser
                .browsers()
                .into_iter()
                .filter(|b| browser != CliBrowser::All || target.detected(*b))
                .collect();
            for b in &browsers {
                target
                    .install(*b, &extra_origin)
                    .map_err(|e| failed(format!("cannot register with {}: {e}", b.as_str())))?;
            }
            Ok(browsers.iter().map(|b| target.status(*b)).collect())
        }
        BridgeCmd::Uninstall { browser } => {
            let browsers = browser.browsers();
            for b in &browsers {
                target
                    .uninstall(*b)
                    .map_err(|e| failed(format!("cannot unregister from {}: {e}", b.as_str())))?;
            }
            Ok(browsers.iter().map(|b| target.status(*b)).collect())
        }
    }
}

fn failed(message: impl Into<String>) -> CliError {
    CliError::new(ExitCode::Generic, IpcError::Internal(message.into()))
}

/// Human-mode rendering: one row per browser.
fn render(statuses: &[BrowserStatus]) -> String {
    let yes = |b: bool| if b { "yes" } else { "no" }.to_string();
    let rows: Vec<Vec<String>> = statuses
        .iter()
        .map(|s| {
            vec![
                s.browser.as_str().to_string(),
                yes(s.detected),
                yes(s.registered),
                yes(s.points_here),
                s.manifest_path.clone(),
            ]
        })
        .collect();
    table(
        &["BROWSER", "FOUND", "REGISTERED", "CURRENT", "MANIFEST"],
        &rows,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::register::{Browser, Platform};

    fn target(root: &std::path::Path) -> Target {
        let bridge = root.join("bin").join(crate::bridge::launch::BRIDGE_EXE);
        std::fs::create_dir_all(bridge.parent().unwrap()).unwrap();
        std::fs::write(&bridge, b"#!").unwrap();
        Target {
            platform: Platform::Linux,
            home: root.join("home"),
            config_home: root.join("home/.config"),
            bridge,
        }
    }

    #[test]
    fn fr_127_install_all_registers_the_browsers_found() {
        let tmp = tempfile::tempdir().unwrap();
        let t = target(tmp.path());
        std::fs::create_dir_all(t.config_home.join("chromium")).unwrap();
        let out = run_at(
            &t,
            BridgeCmd::Install {
                browser: CliBrowser::All,
                extra_origin: vec![],
            },
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].browser, Browser::Chromium);
        assert!(out[0].registered && out[0].points_here);
        // A named browser is registered even when it was not found.
        let out = run_at(
            &t,
            BridgeCmd::Install {
                browser: CliBrowser::Edge,
                extra_origin: vec![format!("chrome-extension://{}/", "e".repeat(32))],
            },
        )
        .unwrap();
        // (Writing its manifest creates the browser's folder, so it now
        // counts as found.)
        assert!(out[0].registered && out[0].points_here);
        assert_eq!(out[0].allowed_origins.len(), 2);

        let status = run_at(&t, BridgeCmd::Status).unwrap();
        assert_eq!(status.len(), 4);
        let registered: Vec<Browser> = status
            .iter()
            .filter(|s| s.registered)
            .map(|s| s.browser)
            .collect();
        assert_eq!(registered, [Browser::Edge, Browser::Chromium]);

        run_at(
            &t,
            BridgeCmd::Uninstall {
                browser: CliBrowser::All,
            },
        )
        .unwrap();
        assert!(run_at(&t, BridgeCmd::Status)
            .unwrap()
            .iter()
            .all(|s| !s.registered));
        assert!(render(&status).starts_with("BROWSER"));
    }

    #[test]
    fn fr_127_a_bad_origin_is_a_usage_error_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let t = target(tmp.path());
        let err = run_at(
            &t,
            BridgeCmd::Install {
                browser: CliBrowser::Chrome,
                extra_origin: vec!["chrome-extension://*/".into()],
            },
        )
        .unwrap_err();
        assert_eq!(err.exit, ExitCode::Usage);
        assert!(!t.status(Browser::Chrome).registered);
    }

    #[test]
    fn fr_127_install_needs_the_bridge_next_to_the_cli() {
        let tmp = tempfile::tempdir().unwrap();
        let mut t = target(tmp.path());
        t.bridge = tmp
            .path()
            .join("missing")
            .join(crate::bridge::launch::BRIDGE_EXE);
        let err = run_at(
            &t,
            BridgeCmd::Install {
                browser: CliBrowser::Chrome,
                extra_origin: vec![],
            },
        )
        .unwrap_err();
        assert_eq!(err.exit, ExitCode::Generic);
    }
}
