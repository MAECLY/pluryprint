//! The JSON handoff file: how a shell extension hands a selection to the app.
//!
//! Windows caps a command line at 32,767 characters, so a selection of a few
//! hundred files cannot travel in argv. The shell extension therefore writes
//! the selection to `<data_dir>/handoff/sel-<uuid>.json` and launches
//! `pluryprint --invoke=context-menu --files-from <path>`; the only variable
//! part of that command line is one path. There is deliberately **no**
//! "small selections go through argv" shortcut: one code path is one code path
//! to test.
//!
//! Anything under the handoff directory is written by another process, so this
//! module treats the file as untrusted input: the schema version must match, the
//! file and the path list are size-capped, and a context-menu payload must be
//! fresh ([`is_fresh`]) and live inside the expected directory
//! ([`is_within_dir`]) before it is honoured. Everything here is pure except
//! [`read_file`]/[`consume`]/[`sweep`], which are the only functions that touch
//! the disk.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::cli::exit::{CliError, ExitCode};
use crate::util::error::IpcError;
use crate::util::paths;

/// On-disk schema version of a handoff file. A payload with any other version
/// is refused rather than guessed at.
pub const HANDOFF_SCHEMA_VERSION: u32 = 1;

/// Largest handoff file we will read. Anything bigger is a bug or an attack,
/// and reading it would mean allocating whatever the writer felt like.
///
/// It has to hold a full [`MAX_HANDOFF_PATHS`] selection of real paths. It
/// used to be 4 MiB, but JSON doubles every backslash and accented folder
/// names are multi-byte, so 10,000 paths under a deep OneDrive folder
/// (~420 bytes each) went over it and Explorer printed nothing at all for a
/// large selection. 64 MiB is ~6.7 KB per path at the cap; the shell
/// extension trims a selection by this same byte budget as well as by count,
/// so a payload it writes is never refused for size.
pub const MAX_HANDOFF_BYTES: u64 = 64 * 1024 * 1024;

/// Largest selection we will honour.
pub const MAX_HANDOFF_PATHS: usize = 10_000;

/// How old a context-menu payload may be before it is refused, which stops an
/// old file from being replayed later.
pub const MAX_HANDOFF_AGE: Duration = Duration::from_secs(60);

/// How long an unconsumed handoff file survives in the handoff directory before
/// [`sweep`] deletes it.
pub const ORPHAN_HANDOFF_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The selection a shell extension serialized for us.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    /// Payload format version; must equal [`HANDOFF_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Which integration wrote the file, e.g. `windows-explorer`, `macos-services`.
    #[serde(default)]
    pub source: String,
    /// Requested action. Optional so a payload that says nothing can fall back
    /// to the `--action` flag instead of silently claiming "configure";
    /// an *unknown* value is a parse error, never a silent default.
    #[serde(default)]
    pub action: Option<super::args::ContextAction>,
    /// Profile to apply, when the menu entry named one (overrides `--profile`).
    #[serde(default)]
    pub profile_id: Option<String>,
    /// RFC 3339 UTC timestamp of the invocation.
    #[serde(default)]
    pub invoked_at: String,
    /// The selected files and folders, as the shell reported them.
    pub paths: Vec<String>,
    /// How many items the user actually selected, when the writer had to cut
    /// the selection down to [`MAX_HANDOFF_PATHS`]. `None` (or absent, for an
    /// older writer) means `paths` is the whole selection. Carried through to
    /// [`super::Selection::truncated_from`] so the user is told (FR-005: nothing
    /// is dropped silently).
    #[serde(default)]
    pub truncated_from: Option<u32>,
    /// A display label for the batch (the browser bridge sends Pluryfact's,
    /// FR-124). Optional and additive: schema version 1 readers that predate
    /// it ignore nothing they relied on, and writers that do not know it
    /// simply leave it out. At most
    /// [`super::MAX_SELECTION_LABEL_CHARS`] characters, no control characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Print settings layered on the profile (the browser bridge's `beginJob`
    /// overrides, FR-123). Optional and additive like `label`; validated by
    /// [`parse`] like every other field, since the file is untrusted input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<super::SelectionOverrides>,
}

/// Directory the shell extensions write handoff files to. Under the per-user
/// data dir (not a world-writable temp dir) so another user cannot plant a
/// payload.
pub fn handoff_dir() -> PathBuf {
    paths::data_dir().join("handoff")
}

/// Parse and structurally validate a handoff payload.
///
/// Pure: the caller decides where the bytes came from. Rejects a wrong schema
/// version, an empty selection and an over-long one — all as
/// [`ExitCode::Usage`], because from the CLI's point of view the *invocation*
/// is malformed.
pub fn parse(raw: &str) -> Result<Handoff, CliError> {
    let handoff: Handoff = serde_json::from_str(raw)
        .map_err(|e| CliError::usage(format!("handoff file is not valid JSON: {e}")))?;

    if handoff.schema_version != HANDOFF_SCHEMA_VERSION {
        return Err(CliError::usage(format!(
            "handoff schema version {} is not supported (expected {HANDOFF_SCHEMA_VERSION})",
            handoff.schema_version
        )));
    }
    if handoff.paths.is_empty() {
        return Err(CliError::usage(
            "handoff file lists no files to print".to_string(),
        ));
    }
    if handoff.paths.len() > MAX_HANDOFF_PATHS {
        return Err(CliError::usage(format!(
            "handoff file lists {} paths, more than the {MAX_HANDOFF_PATHS} limit",
            handoff.paths.len()
        )));
    }
    if let Some(label) = &handoff.label {
        super::validate_label(label).map_err(|e| CliError::usage(format!("handoff file: {e}")))?;
    }
    if let Some(overrides) = &handoff.overrides {
        overrides
            .validate()
            .map_err(|e| CliError::usage(format!("handoff file: {e}")))?;
    }
    Ok(handoff)
}

/// Read and parse the handoff file at `path`.
///
/// A missing/unreadable file is [`ExitCode::FileNotFound`]; an oversized one is
/// refused *before* it is read into memory.
pub fn read_file(path: &Path) -> Result<Handoff, CliError> {
    let meta = std::fs::metadata(path).map_err(|_| {
        CliError::new(
            ExitCode::FileNotFound,
            IpcError::FileNotFound(path.display().to_string()),
        )
    })?;
    if meta.len() > MAX_HANDOFF_BYTES {
        return Err(CliError::usage(format!(
            "handoff file is larger than the {MAX_HANDOFF_BYTES} byte limit"
        )));
    }
    let raw = std::fs::read_to_string(path).map_err(|e| {
        CliError::new(
            ExitCode::FileNotFound,
            IpcError::FileNotFound(format!("{}: {e}", path.display())),
        )
    })?;
    parse(&raw)
}

/// Read a handoff file and delete it. Only for the caller that both writes and
/// reads the file inside one process (the macOS services provider): there is no
/// second reader, so the payload can go the moment it has been parsed.
///
/// A launch that may be *forwarded* to a running instance must use
/// [`read_file`] and leave the deletion to whoever ends up owning the selection
/// — see [`discard`].
pub fn consume(path: &Path) -> Result<Handoff, CliError> {
    let handoff = read_file(path);
    discard(path);
    handoff
}

/// Delete a handoff file that has served its purpose.
///
/// Deletion is deliberately *not* part of reading. A context-menu launch reads
/// the file and then hands its argv to whatever process will actually show the
/// window — itself, or the already-running instance the single-instance plugin
/// found. Only that process knows the selection is safely parked, so only it
/// may delete the file; a reader that deleted eagerly would leave the second
/// reader with nothing (NFR-014).
///
/// Once the selection *is* parked the file is pure leftovers: it names the
/// user's documents and could be replayed inside the freshness window. Failure
/// is logged and swallowed — the selection has already been delivered, and
/// [`sweep`] collects anything left behind.
pub fn discard(path: &Path) {
    // Only the file name: the full path sits under the user's profile, and
    // NFR-082 keeps absolute paths out of the default-level log.
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match std::fs::remove_file(path) {
        Ok(()) => tracing::debug!(event = "handoff_discarded", file = %name),
        Err(e) => tracing::warn!(
            event = "handoff_discard_failed",
            file = %name,
            error = %e,
            "the handoff file will be collected by the orphan sweep instead"
        ),
    }
}

/// How far into the future a timestamp may sit before it is treated as bogus.
/// The payload is written by a *different* process a moment earlier, so a small
/// clock jitter is expected; anything beyond this is not a plausible "just
/// happened".
pub const MAX_HANDOFF_SKEW: Duration = Duration::from_secs(5);

/// Whether `invoked_at` (RFC 3339) is no older than `max_age` relative to
/// `now`. A payload we cannot date — unparseable, or dated well into the
/// future — is *not* fresh. `now` is injected so the check is testable.
pub fn is_fresh(invoked_at: &str, now: time::OffsetDateTime, max_age: Duration) -> bool {
    let Ok(stamp) =
        time::OffsetDateTime::parse(invoked_at, &time::format_description::well_known::Rfc3339)
    else {
        return false;
    };
    let age = now - stamp;
    let max_age = time::Duration::try_from(max_age).unwrap_or(time::Duration::MAX);
    let skew = time::Duration::try_from(MAX_HANDOFF_SKEW).unwrap_or(time::Duration::ZERO);
    age >= -skew && age <= max_age
}

/// Whether `path` resolves inside `dir`. Both sides are canonicalized, so
/// `..` traversal and symlinks cannot smuggle a payload in from elsewhere. A
/// path that cannot be canonicalized (it does not exist) is not inside.
pub fn is_within_dir(dir: &Path, path: &Path) -> bool {
    match (dir.canonicalize(), path.canonicalize()) {
        (Ok(dir), Ok(path)) => path.starts_with(dir),
        _ => false,
    }
}

/// Delete handoff files older than `max_age` from `dir`. Called at GUI startup
/// so a crash between "extension wrote the file" and "app read it" cannot leave
/// the directory growing forever. Best effort: an unreadable directory or a
/// file we cannot stat is skipped silently.
pub fn sweep(dir: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // no handoff dir yet — nothing to sweep
    };
    let now = SystemTime::now();
    let mut removed = 0u32;
    for entry in entries.flatten() {
        let path = entry.path();
        let too_old = entry
            .metadata()
            .and_then(|m| m.modified())
            .and_then(|m| now.duration_since(m).map_err(std::io::Error::other))
            .map(|age| age > max_age)
            .unwrap_or(false);
        if too_old && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        tracing::info!(event = "handoff_orphans_swept", removed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::ContextAction;

    fn payload(paths: &str) -> String {
        format!(
            r#"{{"schema_version":1,"source":"windows-explorer","action":"configure",
                "profile_id":null,"invoked_at":"2026-07-20T21:11:03Z","paths":[{paths}]}}"#
        )
    }

    #[test]
    fn fr_008_parses_a_well_formed_payload() {
        let h = parse(&payload(r#""C:\\a.pdf","C:\\name, with \"quotes\".png""#)).unwrap();
        assert_eq!(h.schema_version, HANDOFF_SCHEMA_VERSION);
        assert_eq!(h.action, Some(ContextAction::Configure));
        assert_eq!(h.paths.len(), 2);
        // The exact reason the format is JSON: separators inside file names.
        assert_eq!(h.paths[1], r#"C:\name, with "quotes".png"#);
    }

    /// FR-123/FR-124: the browser bridge's additive fields (a label and the
    /// per-batch overrides) are read, and a payload that predates them still
    /// parses with both absent.
    #[test]
    fn fr_123_label_and_overrides_are_optional_and_parsed() {
        let h = parse(&payload(r#""/tmp/a.pdf""#)).unwrap();
        assert_eq!((h.label, h.overrides), (None, None));

        let raw = r#"{"schema_version":1,"source":"pluryfact","action":"queue",
            "invoked_at":"2026-07-20T21:11:03Z","paths":["/tmp/a.pdf"],
            "label":"Recibidos · octubre 2026",
            "overrides":{"printer":"HP_LaserJet","copies":2,"duplex":"long-edge",
                         "color":"monochrome","paper":"a4","orientation":"portrait",
                         "collate":false}}"#;
        let h = parse(raw).unwrap();
        assert_eq!(h.label.as_deref(), Some("Recibidos · octubre 2026"));
        let o = h.overrides.unwrap();
        assert_eq!(o.printer.as_deref(), Some("HP_LaserJet"));
        assert_eq!(o.copies, Some(2));
        assert_eq!(o.duplex, Some(crate::profiles::ProfileDuplex::LongEdge));
        assert_eq!(o.color, Some(crate::queue::job::ColorMode::Monochrome));
        assert_eq!(o.paper, Some(crate::queue::job::PaperSize::A4));
        assert_eq!(
            o.orientation,
            Some(crate::queue::job::Orientation::Portrait)
        );
        assert_eq!(o.collate, Some(false));
    }

    /// FR-123: the file is untrusted, so the new fields are checked like the
    /// old ones: copies out of range, an unknown override, a control
    /// character or an over-long label refuse the whole payload.
    #[test]
    fn fr_123_bad_overrides_or_label_refuse_the_payload() {
        let with = |extra: &str| {
            format!(
                r#"{{"schema_version":1,"source":"pluryfact","invoked_at":"2026-07-20T21:11:03Z",
                    "paths":["/tmp/a.pdf"],{extra}}}"#
            )
        };
        for extra in [
            r#""overrides":{"copies":0}"#,
            r#""overrides":{"copies":1000}"#,
            r#""overrides":{"scale":"fit"}"#,
            r#""overrides":{"duplex":"both"}"#,
            r#""overrides":{"printer":"  "}"#,
            r#""overrides":{"printer":"ab"}"#,
            r#""label":"line
break""#,
        ] {
            assert_eq!(
                parse(&with(extra)).unwrap_err().exit,
                ExitCode::Usage,
                "{extra}"
            );
        }
        let long = format!(
            r#""label":"{}""#,
            "x".repeat(super::super::MAX_SELECTION_LABEL_CHARS + 1)
        );
        assert_eq!(parse(&with(&long)).unwrap_err().exit, ExitCode::Usage);
        let just = format!(
            r#""label":"{}""#,
            "ñ".repeat(super::super::MAX_SELECTION_LABEL_CHARS)
        );
        assert!(
            parse(&with(&just)).is_ok(),
            "the limit counts characters, not bytes"
        );
    }

    #[test]
    fn fr_008_rejects_corrupt_json_and_wrong_schema() {
        assert_eq!(parse("{not json").unwrap_err().exit, ExitCode::Usage);
        let future = r#"{"schema_version":99,"paths":["/a.pdf"]}"#;
        assert_eq!(parse(future).unwrap_err().exit, ExitCode::Usage);
    }

    #[test]
    fn fr_008_rejects_empty_and_oversized_selections() {
        assert_eq!(parse(&payload("")).unwrap_err().exit, ExitCode::Usage);

        let many = (0..MAX_HANDOFF_PATHS + 1)
            .map(|i| format!("\"/tmp/f{i}.pdf\""))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(parse(&payload(&many)).unwrap_err().exit, ExitCode::Usage);
    }

    #[test]
    fn fr_008_accepts_a_large_but_legal_selection() {
        let many = (0..2_000)
            .map(|i| format!("\"/tmp/f{i}.pdf\""))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(parse(&payload(&many)).unwrap().paths.len(), 2_000);
    }

    /// FR-003: the shell extension sends up to 10,000 paths. With deep,
    /// accented OneDrive folders (~420 JSON bytes a path) that is over the old
    /// 4 MiB read cap, and the whole selection was refused.
    #[test]
    fn fr_003_a_full_selection_of_long_paths_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let folder = format!(
            r"C:\\Users\\ana\\OneDrive - Empresa Ejemplo\\Facturación {}",
            "ñ".repeat(200)
        );
        let many = (0..MAX_HANDOFF_PATHS)
            .map(|i| format!("\"{folder}\\\\factura-{i:05}.pdf\""))
            .collect::<Vec<_>>()
            .join(",");
        let body = payload(&many);
        assert!(body.len() > 4 * 1024 * 1024, "{} bytes", body.len());
        let file = dir.path().join("sel.json");
        std::fs::write(&file, &body).unwrap();

        let h = read_file(&file).expect("a legal selection is read whatever its size");
        assert_eq!(h.paths.len(), MAX_HANDOFF_PATHS);
    }

    #[test]
    fn fr_008_missing_file_is_file_not_found() {
        let err = read_file(Path::new("/no/such/handoff.json")).unwrap_err();
        assert_eq!(err.exit, ExitCode::FileNotFound);
    }

    #[test]
    fn fr_008_consume_deletes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sel.json");
        std::fs::write(&path, payload(r#""/tmp/a.pdf""#)).unwrap();

        assert_eq!(consume(&path).unwrap().paths, vec!["/tmp/a.pdf"]);
        assert!(!path.exists(), "a consumed handoff file is deleted");
    }

    /// The other half of the same rule: reading leaves the file alone, so a
    /// launch that is about to be forwarded does not destroy the only copy of
    /// the selection before the running instance has read it (NFR-014).
    #[test]
    fn nfr_014_reading_leaves_the_file_for_the_next_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sel.json");
        std::fs::write(&path, payload(r#""/tmp/a.pdf""#)).unwrap();

        assert_eq!(read_file(&path).unwrap().paths, vec!["/tmp/a.pdf"]);
        assert!(path.exists(), "read_file must not consume the payload");
        assert_eq!(read_file(&path).unwrap().paths, vec!["/tmp/a.pdf"]);

        discard(&path);
        assert!(!path.exists(), "discard is what deletes it");
        // Discarding what is already gone is a no-op, not a panic: two
        // processes can both believe they own the selection.
        discard(&path);
    }

    #[test]
    fn nfr_014_freshness_bounds_a_replayed_payload() {
        let now = time::OffsetDateTime::parse(
            "2026-07-20T21:12:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        assert!(is_fresh("2026-07-20T21:11:30Z", now, MAX_HANDOFF_AGE));
        assert!(!is_fresh("2026-07-20T21:00:00Z", now, MAX_HANDOFF_AGE));
        // A second of clock jitter between the two processes is tolerated…
        assert!(is_fresh("2026-07-20T21:12:01Z", now, MAX_HANDOFF_AGE));
        // …a minute into the future is not, and neither is an undateable stamp.
        assert!(!is_fresh("2026-07-20T21:13:00Z", now, MAX_HANDOFF_AGE));
        assert!(!is_fresh("yesterday", now, MAX_HANDOFF_AGE));
    }

    #[test]
    fn nfr_014_containment_rejects_a_payload_from_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("sel.json");
        std::fs::write(&inside, "{}").unwrap();
        assert!(is_within_dir(dir.path(), &inside));

        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("sel.json");
        std::fs::write(&outside, "{}").unwrap();
        assert!(!is_within_dir(dir.path(), &outside));
        assert!(!is_within_dir(dir.path(), &dir.path().join("gone.json")));
    }

    #[test]
    fn nfr_014_sweep_keeps_recent_files() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("sel.json");
        std::fs::write(&fresh, "{}").unwrap();

        sweep(dir.path(), ORPHAN_HANDOFF_AGE);
        assert!(fresh.exists(), "a just-written handoff survives the sweep");

        // A zero-age cutoff deletes everything, proving the age gate is what
        // keeps the file above.
        sweep(dir.path(), Duration::ZERO);
        assert!(!fresh.exists());
        // A missing directory is not an error.
        sweep(&dir.path().join("nope"), ORPHAN_HANDOFF_AGE);
    }
}
