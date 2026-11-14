//! The bridge's inbox: one folder per job under `<data_dir>/inbox/`, holding
//! the documents in print order (`0000-name.pdf`, `0001-…`).
//!
//! The folder is per user (the app's data directory), created with the
//! default permissions of that directory, and only ever holds what the
//! bridge itself wrote. A job folder is deleted when the job is aborted or
//! the connection closes before `commitJob`; a committed job's folder is what
//! the app prints from, so it stays until the app's startup [`sweep`] finds
//! it older than [`super::INBOX_RETENTION`] (FR-126).

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Whether `job_id` is safe to use as a folder name: the bridge mints ids
/// itself (UUID v7), so anything else — separators, dots, a drive letter —
/// is refused before it gets near a path.
pub fn is_valid_job_id(job_id: &str) -> bool {
    !job_id.is_empty()
        && job_id.len() <= 64
        && job_id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// The folder of `job_id` under `inbox`. `None` for an id
/// [`is_valid_job_id`] refuses.
pub fn job_dir(inbox: &Path, job_id: &str) -> Option<PathBuf> {
    is_valid_job_id(job_id).then(|| inbox.join(job_id))
}

/// Create a new, empty job folder. Fails if it already exists, so two jobs
/// can never share one.
pub fn create_job_dir(inbox: &Path, job_id: &str) -> io::Result<PathBuf> {
    let dir = job_dir(inbox, job_id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid job id"))?;
    std::fs::create_dir_all(inbox)?;
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// Delete a job folder and everything in it. A folder that is already gone
/// is not an error.
pub fn remove_job_dir(dir: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Delete every entry of `inbox` last modified more than `max_age` ago, and
/// report how many went. Called at app startup with
/// [`super::INBOX_RETENTION`].
///
/// A symbolic link is removed as a link, never followed: the inbox only ever
/// holds folders the bridge created, and a link planted there must not turn
/// the sweep into "delete whatever it points at". Best effort: an entry that
/// cannot be inspected or removed is skipped.
pub fn sweep(inbox: &Path, max_age: Duration) -> u32 {
    sweep_at(inbox, max_age, SystemTime::now())
}

/// [`sweep`] with an injected clock, for tests.
pub fn sweep_at(inbox: &Path, max_age: Duration, now: SystemTime) -> u32 {
    let Ok(entries) = std::fs::read_dir(inbox) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > max_age);
        if !old {
            continue;
        }
        let gone = if meta.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            // Files and links alike: `remove_file` unlinks a symlink itself.
            std::fs::remove_file(&path)
        };
        if gone.is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        tracing::info!(event = "bridge_inbox_swept", removed);
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr_126_job_ids_are_folder_safe() {
        assert!(is_valid_job_id("019a3b4c-5d6e-7f80-9a1b-2c3d4e5f6a7b"));
        for bad in [
            "",
            "..",
            "../x",
            "a/b",
            "a\\b",
            "C:",
            "x.y",
            &"a".repeat(65),
            "ñ",
        ] {
            assert!(!is_valid_job_id(bad), "{bad:?}");
            assert!(job_dir(Path::new("/inbox"), bad).is_none());
        }
    }

    #[test]
    fn fr_126_a_job_folder_is_created_once_and_removed_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = tmp.path().join("inbox");
        let dir = create_job_dir(&inbox, "0123-abcd").unwrap();
        assert!(dir.is_dir());
        assert!(create_job_dir(&inbox, "0123-abcd").is_err(), "never shared");
        std::fs::write(dir.join("0000-a.pdf"), b"%PDF-").unwrap();
        remove_job_dir(&dir).unwrap();
        assert!(!dir.exists());
        remove_job_dir(&dir).unwrap();
    }

    #[test]
    fn fr_126_sweep_removes_only_old_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = tmp.path();
        let job = create_job_dir(inbox, "aaaa").unwrap();
        std::fs::write(job.join("0000-a.pdf"), b"%PDF-").unwrap();

        // Seven days: what was just written survives…
        assert_eq!(sweep(inbox, crate::bridge::INBOX_RETENTION), 0);
        assert!(job.exists());
        // …and eight days later it is gone.
        let later = SystemTime::now() + Duration::from_secs(8 * 24 * 60 * 60);
        assert_eq!(sweep_at(inbox, crate::bridge::INBOX_RETENTION, later), 1);
        assert!(!job.exists());
        // A missing inbox is nothing to sweep.
        assert_eq!(sweep(&inbox.join("nope"), Duration::ZERO), 0);
    }

    /// A link planted in the inbox is removed as a link; its target survives.
    #[cfg(unix)]
    #[test]
    fn fr_126_sweep_never_follows_a_link() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = tmp.path().join("inbox");
        std::fs::create_dir(&inbox).unwrap();
        let precious = tmp.path().join("precious");
        std::fs::create_dir(&precious).unwrap();
        std::fs::write(precious.join("keep.txt"), b"keep").unwrap();
        std::os::unix::fs::symlink(&precious, inbox.join("link")).unwrap();

        let later = SystemTime::now() + Duration::from_secs(30 * 24 * 60 * 60);
        assert_eq!(sweep_at(&inbox, crate::bridge::INBOX_RETENTION, later), 1);
        assert!(!inbox.join("link").exists());
        assert!(
            precious.join("keep.txt").is_file(),
            "the target is untouched"
        );
    }
}
