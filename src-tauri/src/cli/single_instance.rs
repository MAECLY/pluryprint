//! Single instance (NFR-014): what the *second* launch hands to the first.
//!
//! Two Pluryprint processes must never run at once. Both would recover the
//! same `queue.json` into their own memory and rewrite it from there, and — the
//! damage that actually loses work — the second one's boot-time orphan sweep
//! (`queue::sweep_orphans`) deletes every cached PDF the first one's live jobs
//! have not yet printed, because they are not referenced by *its* copy of the
//! queue.
//!
//! `tauri-plugin-single-instance` closes that hole: the second process finds
//! the first, hands over its `argv` and exits. What is left to decide is what
//! that argv *means*, which is what [`interpret`] does — and it is a pure
//! function of the argv, so the interesting behaviour is testable without ever
//! starting two processes.
//!
//! Note what does *not* come through here: a headless `pluryprint print …`
//! never builds a Tauri app, so it never registers the plugin and is never
//! forwarded. It also never touches `queue.json` or the shared conversion cache
//! ([`crate::cli::print`]), so it cannot race a running app either.

use std::path::{Path, PathBuf};

use crate::cli::args::Cli;
use crate::cli::{handoff, selection_from_at_cwd, Selection};

/// A forwarded launch this instance accepted.
///
/// The handoff file travels with the selection because it is still on disk:
/// [`crate::cli::selection_from_at`] only reads it, so that the launching
/// process (which read it a moment earlier, before finding out it would be
/// forwarded) cannot have destroyed it. The caller deletes it with
/// [`handoff::discard`] once the selection is parked — see
/// [`crate::run_with`].
#[derive(Debug)]
pub struct Forwarded {
    /// The selection to merge into the running instance.
    pub selection: Selection,
    /// The handoff file it came out of — `None` for an "open with" launch or
    /// a Linux file-manager action, whose paths travel on the command line
    /// itself.
    pub handoff: Option<PathBuf>,
}

/// Interpret the argv a second launch forwarded from its working directory
/// `cwd` (as the single-instance plugin reports it).
pub fn interpret<S: AsRef<str>>(argv: &[S], cwd: &str) -> Option<Forwarded> {
    interpret_at(&handoff::handoff_dir(), argv, cwd)
}

/// Interpret the argv a second launch forwarded, resolving its handoff file
/// against `dir` (injected so the seam is testable against a tempdir).
///
/// Relative "open with" paths are anchored against `cwd`, the *launching*
/// process' working directory: `cd C:\docs && pluryprint-cli report.pdf`
/// while the app runs must open `C:\docs\report.pdf`, not drop it because this
/// process sits in another folder. An empty `cwd` anchors nothing.
///
/// Returns the selection to merge into the running instance, or `None` when the
/// launch carried nothing usable (a plain re-launch, a malformed command line,
/// a stale or foreign handoff file). `None` is not an error: the running window
/// is still raised, which is what the user asked for by launching the app.
pub fn interpret_at<S: AsRef<str>>(dir: &Path, argv: &[S], cwd: &str) -> Option<Forwarded> {
    let argv: Vec<String> = argv.iter().map(|a| a.as_ref().to_string()).collect();

    // A context-menu invocation carries a selection in its handoff file (or,
    // on Linux, in the paths after `--`); an
    // "open with" launch carries absolute paths on the command line (the same
    // rule `startup` applies to a first launch, so a file opened while the app
    // is already running is not silently dropped); anything else is a plain
    // re-launch. Classifying first keeps a stray `--help` in a forwarded argv
    // from being answered by a running GUI.
    //
    // Relative paths are anchored *before* classifying: `classify` asks
    // whether a bare argument exists, and asked from this process' folder a
    // caller-relative `report.pdf` would not, and would read as a typo.
    let cwd = (!cwd.is_empty()).then(|| Path::new(cwd));
    let tail = super::anchor_relative(argv.get(1..).unwrap_or_default(), cwd);
    match super::classify(&tail) {
        super::Intent::ContextMenu => {}
        super::Intent::Gui => {
            return super::open_with_selection(&tail).map(|selection| Forwarded {
                selection,
                handoff: None,
            });
        }
        super::Intent::Headless => return None,
    }

    // The raw argv, not `tail`: anchoring there skips flag-looking
    // arguments, which is right before `classify` but would leave a file
    // named `-a.pdf` after `--` relative. The paths after `--` are anchored
    // against `cwd` by `argv_selection` itself.
    let Some(cli) = Cli::parse_context_menu(&argv).ok() else {
        tracing::warn!(
            event = "second_instance_argv_unparsed",
            "a forwarded launch had an unparseable command line"
        );
        return None;
    };
    match selection_from_at_cwd(dir, &cli, cwd) {
        // A selection read from a handoff file names it, so the caller can
        // retire it; one taken from the paths after `--` (Linux) has none.
        Ok(selection) => Some(Forwarded {
            selection,
            handoff: cli.files_from,
        }),
        Err(err) => {
            tracing::warn!(
                event = "second_instance_selection_rejected",
                error = %err.error,
                "a forwarded launch carried a selection we could not honour"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A handoff file inside `dir`, written the way a shell extension writes
    /// it: fresh, schema-versioned, with an action of its own.
    fn write_handoff(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!(
                r#"{{"schema_version":1,"source":"windows-explorer","action":"print-default",
                    "profile_id":"builtin-quick","invoked_at":"{}",
                    "paths":["/tmp/a.pdf","/tmp/b.pdf"]}}"#,
                crate::queue::job::now_rfc3339()
            ),
        )
        .unwrap();
        path
    }

    fn context_menu_argv(handoff: &Path) -> Vec<String> {
        vec![
            "pluryprint".to_string(),
            "--invoke=context-menu".to_string(),
            "--files-from".to_string(),
            handoff.to_string_lossy().into_owned(),
        ]
    }

    /// The seam this whole module exists for: a forwarded context-menu argv
    /// must actually resolve to a selection. Everything else here asserts a
    /// `None`, which an implementation that never returns anything would also
    /// satisfy — this is the test that says the feature works at all.
    #[test]
    fn nfr_014_a_forwarded_context_menu_argv_resolves_to_its_selection() {
        let dir = tempfile::tempdir().unwrap();
        let handoff = write_handoff(dir.path(), "sel.json");

        let forwarded = interpret_at(dir.path(), &context_menu_argv(&handoff), "")
            .expect("a fresh, contained, well-formed payload is honoured");
        assert_eq!(forwarded.selection.paths, vec!["/tmp/a.pdf", "/tmp/b.pdf"]);
        assert_eq!(
            forwarded.selection.action,
            crate::cli::args::ContextAction::PrintDefault,
            "the payload's own action decides what the app does"
        );
        assert_eq!(
            forwarded.selection.profile_id.as_deref(),
            Some("builtin-quick")
        );
        assert_eq!(
            forwarded.handoff,
            Some(handoff),
            "the caller needs the path to discard it once the selection is parked"
        );
    }

    /// The regression the two-process flow used to hit: the launching process
    /// interprets its own argv, then the plugin forwards that same argv to the
    /// running instance, which interprets it again. Both readings must succeed
    /// — which they only do because reading does not delete.
    #[test]
    fn nfr_014_the_same_argv_survives_being_interpreted_twice() {
        let dir = tempfile::tempdir().unwrap();
        let handoff = write_handoff(dir.path(), "sel.json");
        let argv = context_menu_argv(&handoff);

        let first = interpret_at(dir.path(), &argv, "").expect("the launching process reads it");
        assert!(
            handoff.exists(),
            "reading must leave the file for the forward"
        );

        let second =
            interpret_at(dir.path(), &argv, "").expect("the running instance reads it too");
        assert_eq!(first.selection, second.selection);

        // The instance that parked the selection is the one that deletes it,
        // and only then does the argv stop resolving.
        crate::cli::handoff::discard(second.handoff.as_ref().unwrap());
        assert!(!handoff.exists());
        assert!(interpret_at(dir.path(), &argv, "").is_none());
    }

    #[test]
    fn nfr_014_plain_relaunch_carries_no_selection() {
        assert!(interpret(&["pluryprint"], "").is_none());
        assert!(interpret(&["pluryprint", "-psn_0_1234"], "").is_none());
        // A subcommand can never reach this path (headless never builds an
        // app), and if one somehow did we must not act on it.
        assert!(interpret(&["pluryprint", "print", "--files", "a.pdf"], "").is_none());
    }

    /// A file opened "with" Pluryprint while it is already running reaches
    /// the running window as a `configure` selection instead of vanishing.
    #[test]
    fn fr_008_a_forwarded_open_with_launch_carries_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("report.pdf");
        std::fs::write(&file, b"%PDF-1.4\n").unwrap();

        let forwarded = interpret_at(
            dir.path(),
            &[
                "pluryprint".to_string(),
                file.to_string_lossy().into_owned(),
            ],
            "",
        )
        .expect("an absolute, existing path is honoured");
        assert_eq!(
            forwarded.selection.paths,
            vec![crate::util::paths::canonical_input(&file.to_string_lossy())
                .unwrap()
                .to_string_lossy()
                .into_owned()]
        );
        assert_eq!(
            forwarded.selection.action,
            crate::cli::args::ContextAction::Configure
        );
        assert!(forwarded.handoff.is_none());
    }

    #[test]
    fn nfr_014_context_menu_without_a_handoff_is_rejected() {
        assert!(interpret(&["pluryprint", "--invoke=context-menu"], "").is_none());
    }

    #[test]
    fn nfr_014_foreign_handoff_file_is_rejected() {
        // A payload outside the handoff directory is refused even when it is
        // otherwise perfect: it is the containment check that stops another
        // process from planting one.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sel.json");
        std::fs::write(
            &path,
            format!(
                r#"{{"schema_version":1,"source":"test","invoked_at":"{}","paths":["/tmp/a.pdf"]}}"#,
                crate::queue::job::now_rfc3339()
            ),
        )
        .unwrap();

        let argv = vec![
            "pluryprint".to_string(),
            "--invoke=context-menu".to_string(),
            "--files-from".to_string(),
            path.to_string_lossy().into_owned(),
        ];
        assert!(interpret(&argv, "").is_none());
        assert!(path.exists(), "a refused payload is not consumed");
    }

    /// A Dolphin or Nemo action run while the app is open reaches the running
    /// window with its action, relative paths anchored at the launching
    /// folder, and no handoff file to retire. Linux only: elsewhere the same
    /// argv is refused, as it always was.
    #[test]
    fn fr_004_a_forwarded_file_manager_action_carries_its_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("report.pdf"), b"%PDF-1.4\n").unwrap();
        let argv = [
            "pluryprint",
            "--invoke=context-menu",
            "--action=print-default",
            "--",
            "report.pdf",
        ];

        let forwarded = interpret_at(dir.path(), &argv, &dir.path().to_string_lossy());
        if !crate::cli::ARGV_SELECTION {
            assert!(forwarded.is_none());
            return;
        }
        let forwarded = forwarded.expect("the running instance honours it");
        assert_eq!(
            forwarded.selection.paths,
            vec![crate::util::paths::canonical_input(
                &dir.path().join("report.pdf").to_string_lossy()
            )
            .unwrap()
            .to_string_lossy()
            .into_owned()]
        );
        assert_eq!(
            forwarded.selection.action,
            crate::cli::args::ContextAction::PrintDefault
        );
        assert!(forwarded.handoff.is_none());
        // Without the caller's folder the relative path is never guessed.
        assert!(interpret_at(dir.path(), &argv, "").is_none());
    }

    /// FR-008: `cd C:\docs && pluryprint-cli report.pdf` while the app is
    /// open forwards a *relative* path. It is resolved against the launching
    /// terminal's folder, which the plugin reports, not dropped.
    #[test]
    fn fr_008_a_forwarded_relative_path_is_anchored_at_the_callers_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("report.pdf"), b"%PDF-1.4\n").unwrap();
        let argv = ["pluryprint".to_string(), "report.pdf".to_string()];

        let forwarded = interpret_at(dir.path(), &argv, &dir.path().to_string_lossy())
            .expect("the relative path resolves against the caller's folder");
        assert_eq!(
            forwarded.selection.paths,
            vec![crate::util::paths::canonical_input(
                &dir.path().join("report.pdf").to_string_lossy()
            )
            .unwrap()
            .to_string_lossy()
            .into_owned()]
        );
        // Without the caller's folder a relative path is still never guessed.
        assert!(interpret_at(dir.path(), &argv, "").is_none());
    }
}
