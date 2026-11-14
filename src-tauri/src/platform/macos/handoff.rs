//! Writing the handoff file — the producer side of the shell-extension contract.
//!
//! [`crate::cli::handoff`] owns the *consumer* side: the schema, the caps and
//! the validation rules. This module is the only place that produces a file in
//! that format on macOS, and it deliberately builds
//! [`Handoff`](crate::cli::handoff::Handoff) — the same struct the reader
//! deserializes — instead of hand-rolling JSON, so the two sides cannot drift.
//!
//! Why write a file at all when the Finder service is answered *inside* the
//! running app (see [`super::services`])? Three reasons, all about having one
//! contract rather than two:
//!
//! - the selection then travels through exactly the same validation as a
//!   Windows Explorer invocation (schema, freshness, containment, path caps),
//!   so a bug in that gate is caught on the platform we can actually run;
//! - the file is the artefact a test can assert on, which makes the Finder
//!   path testable without AppKit;
//! - if the app is ever launched by Launch Services rather than being already
//!   alive, the same bytes are what a `--files-from` relaunch would carry.
//!
//! The write is atomic (temp file + rename) because the reader may be a
//! different process, and it must never observe a half-written selection.

use std::io;
use std::path::{Path, PathBuf};

use crate::cli::args::ContextAction;
use crate::cli::handoff::{Handoff, HANDOFF_SCHEMA_VERSION, MAX_HANDOFF_PATHS};

/// Value of the payload's `source` field for a Finder Services invocation.
/// Windows Explorer writes `windows-explorer`; the field exists so a log line
/// or a bug report says which integration produced the file.
pub const SOURCE: &str = "macos-services";

/// Build the payload for a selection. Pure, so the exact bytes the Finder path
/// produces can be asserted without touching the disk or AppKit.
///
/// `invoked_at` is injected rather than read from the clock for the same
/// reason [`crate::cli::handoff::is_fresh`] takes a `now`: freshness is a rule
/// that must be testable in both directions.
pub fn payload(
    paths: &[PathBuf],
    action: ContextAction,
    profile_id: Option<String>,
    invoked_at: time::OffsetDateTime,
) -> Handoff {
    Handoff {
        schema_version: HANDOFF_SCHEMA_VERSION,
        source: SOURCE.to_string(),
        action: Some(action),
        profile_id,
        invoked_at: invoked_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        // `to_string_lossy` cannot corrupt a real selection: the paths come
        // from `NSURL.path`, which is already valid UTF-8.
        paths: paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        // The Services provider hands over whatever the Finder passed.
        truncated_from: None,
        label: None,
        overrides: None,
    }
}

/// Serialize `handoff` into `dir` and return the file's path.
///
/// Refuses a selection larger than the reader will accept ([`MAX_HANDOFF_PATHS`])
/// rather than writing a file that is guaranteed to be rejected on the other
/// side — the caller can then say something useful instead of the user watching
/// a window open empty.
pub fn write(dir: &Path, handoff: &Handoff) -> io::Result<PathBuf> {
    if handoff.paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to write an empty selection",
        ));
    }
    if handoff.paths.len() > MAX_HANDOFF_PATHS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "selection of {} files is over the {MAX_HANDOFF_PATHS} limit",
                handoff.paths.len()
            ),
        ));
    }

    std::fs::create_dir_all(dir)?;
    restrict(dir);

    let name = format!("sel-{}.json", uuid::Uuid::now_v7().simple());
    let path = dir.join(&name);
    let tmp = dir.join(format!("{name}.tmp"));

    let bytes = serde_json::to_vec(handoff).map_err(io::Error::other)?;
    // Temp file first, then rename: `rename` is atomic within a directory, so a
    // reader either sees the whole selection or no file at all. A failure after
    // the write leaves a `.tmp` behind, which the startup sweep collects.
    std::fs::write(&tmp, &bytes)?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(path)
}

/// Best effort `chmod 0700` on the handoff directory.
///
/// The directory already lives under the per-user Application Support tree, so
/// this is defence in depth rather than the primary control — but the files
/// name every document the user just selected, and on a shared Mac the default
/// umask leaves `~/Library` group-readable.
#[cfg(unix)]
fn restrict(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict(_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::handoff;

    fn now() -> time::OffsetDateTime {
        time::OffsetDateTime::now_utc()
    }

    fn paths(n: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| PathBuf::from(format!("/tmp/f{i}.pdf")))
            .collect()
    }

    /// FR-001: what the Finder path writes is exactly what the app's own reader
    /// accepts — same schema, same action vocabulary, and fresh.
    #[test]
    fn fr_001_a_written_selection_round_trips_through_the_reader() {
        let dir = tempfile::tempdir().unwrap();
        let written = payload(
            &[PathBuf::from("/tmp/a.pdf")],
            ContextAction::PrintDefault,
            Some("quick".to_string()),
            now(),
        );
        let file = write(dir.path(), &written).unwrap();

        let read = handoff::consume(&file).unwrap();
        assert_eq!(read, written);
        assert_eq!(read.source, SOURCE);
        assert_eq!(read.action, Some(ContextAction::PrintDefault));
        assert!(handoff::is_fresh(
            &read.invoked_at,
            now(),
            handoff::MAX_HANDOFF_AGE
        ));
        assert!(!file.exists(), "the reader consumes the file it read");
    }

    /// FR-003: no 15-file ceiling. A selection far past what Explorer's legacy
    /// verb would allow survives the round trip untouched.
    #[test]
    fn fr_003_a_large_selection_survives_the_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let many = paths(5_000);
        let file = write(
            dir.path(),
            &payload(&many, ContextAction::Configure, None, now()),
        )
        .unwrap();

        let read = handoff::consume(&file).unwrap();
        assert_eq!(read.paths.len(), 5_000);
        assert_eq!(read.paths[4_999], "/tmp/f4999.pdf");
    }

    /// The producer refuses what the consumer would refuse, so an over-large
    /// selection fails where it can still be reported instead of silently
    /// producing a file that is rejected in another process.
    #[test]
    fn fr_003_the_writer_enforces_the_readers_caps() {
        let dir = tempfile::tempdir().unwrap();
        let over = payload(
            &paths(MAX_HANDOFF_PATHS + 1),
            ContextAction::Configure,
            None,
            now(),
        );
        assert_eq!(
            write(dir.path(), &over).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        let empty = payload(&[], ContextAction::Configure, None, now());
        assert_eq!(
            write(dir.path(), &empty).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "a refused selection leaves no file behind"
        );
    }

    /// FR-004/FR-005: names the shell can produce but a delimited list cannot
    /// carry — separators, quotes, newlines, non-ASCII — travel intact. This is
    /// the whole reason the transport is JSON rather than a comma-joined argv.
    #[test]
    fn fr_004_hostile_file_names_survive_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let nasty = vec![
            PathBuf::from("/tmp/name, with comma.pdf"),
            PathBuf::from("/tmp/say \"hello\".docx"),
            PathBuf::from("/tmp/two\nlines.png"),
            PathBuf::from("/tmp/back\\slash.pdf"),
            PathBuf::from("/tmp/facturas ñ 2026 🖨.pdf"),
            PathBuf::from("/tmp/a folder with spaces"),
        ];
        let file = write(
            dir.path(),
            &payload(&nasty, ContextAction::Configure, None, now()),
        )
        .unwrap();

        let read = handoff::consume(&file).unwrap();
        let expected: Vec<String> = nasty
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(read.paths, expected);
    }

    /// The written file lands inside the directory the reader will insist on,
    /// which is the containment check `cli::selection_from` applies (NFR-014).
    #[test]
    fn nfr_014_the_file_is_written_inside_the_handoff_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(
            dir.path(),
            &payload(&paths(1), ContextAction::Configure, None, now()),
        )
        .unwrap();

        assert!(handoff::is_within_dir(dir.path(), &file));
        assert_eq!(file.extension().and_then(|e| e.to_str()), Some("json"));
        // No `.tmp` left over: the rename is the last step and it succeeded.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// Two invocations in the same millisecond must not overwrite each other:
    /// each selection gets its own file (uuid v7 names).
    #[test]
    fn nfr_014_concurrent_invocations_get_distinct_files() {
        let dir = tempfile::tempdir().unwrap();
        let payload = payload(&paths(1), ContextAction::Configure, None, now());
        let a = write(dir.path(), &payload).unwrap();
        let b = write(dir.path(), &payload).unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn nfr_014_the_handoff_directory_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("handoff");
        write(
            &nested,
            &payload(&paths(1), ContextAction::Configure, None, now()),
        )
        .unwrap();

        let mode = std::fs::metadata(&nested).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o700,
            "handoff dir must not be group/world readable"
        );
    }
}
