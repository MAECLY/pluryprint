//! Turning `--files` entries into a concrete list of paths.
//!
//! Three kinds of entry are accepted, because that is what the documented
//! examples use (`ipc-and-cli.md` §3.3/§3.4):
//!
//! - a plain path — kept as written (existence is checked later, so the error
//!   the user sees names the file they typed);
//! - a directory — handed to [`crate::engine::expand_folder`], the same walk
//!   the print dialog does for a dropped folder (FR-004), so a folder means the
//!   same thing on both surfaces;
//! - a wildcard pattern in the **last** path component (`C:\Reports\*.pdf`) —
//!   expanded here rather than by the shell, so it behaves identically in
//!   cmd.exe and in bash, which is exactly what §3.3 promises.
//!
//! Relative entries are resolved against the working directory *here* and only
//! here: a command line is the one place a relative path is meaningful (the
//! GUI's paths arrive from a picker or a drop and are refused if relative, see
//! [`crate::util::paths::canonical_input`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::cli::exit::CliError;
use crate::engine::{expand_folder, ExpansionLimits, FolderScan};

/// Expand every `--files` entry into concrete paths.
///
/// `max_file_size_mb` is the configured size ceiling, needed because folder
/// expansion filters by it. Patterns and folders that match nothing are an
/// error naming the entry — silently printing zero files would look like
/// success.
///
/// A path that turns up more than once — typed twice, or matched by both a
/// pattern and a folder — is printed once, and in the position of its **first**
/// appearance: the order of `--files` is the order the pages come out, so the
/// list is never sorted to make duplicates adjacent.
pub fn expand_inputs(entries: &[String], max_file_size_mb: u32) -> Result<Vec<String>, CliError> {
    let mut out: Vec<String> = Vec::new();

    for entry in entries {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let path = absolutize(trimmed);

        let expanded = if has_pattern(trimmed) {
            expand_pattern(&path)
        } else if path.is_dir() {
            // A script that points at a folder wants the folder, subfolders
            // included; the interactive "top level or everything?" question
            // (FR-004) has no one to answer it here.
            let walk = expand_folder(
                &path,
                FolderScan::Recursive,
                ExpansionLimits::default(),
                max_file_size_mb,
            );
            if !walk.skipped.is_empty() {
                tracing::debug!(
                    event = "cli_folder_entries_skipped",
                    folder = %path.display(),
                    skipped = walk.skipped.len()
                );
            }
            walk.files
        } else {
            vec![path.to_string_lossy().into_owned()]
        };

        if expanded.is_empty() {
            return Err(CliError::usage(format!("{trimmed} matched no files")));
        }
        out.extend(expanded);
    }

    if out.is_empty() {
        return Err(CliError::usage("no files were provided to print"));
    }
    // Order-preserving: `Vec::dedup` only collapses *adjacent* equals, so on an
    // unsorted list `a.pdf,b.pdf,a.pdf` printed a.pdf twice.
    let mut seen: HashSet<String> = HashSet::with_capacity(out.len());
    out.retain(|path| seen.insert(path.clone()));
    Ok(out)
}

/// Resolve `entry` against the caller's folder ([`crate::cli::caller_dir`]:
/// the working directory, or an AppImage's `OWD`) when it is relative.
/// Nothing is canonicalized yet — a pattern has no file to canonicalize, and
/// a missing file must still be reported by the name the user typed.
fn absolutize(entry: &str) -> PathBuf {
    crate::cli::from_caller(Path::new(entry))
}

/// Whether an entry carries a wildcard in its final component. A pattern in a
/// *directory* component (`C:\**\a.pdf`) is deliberately not supported: it is
/// not in the documented contract and it invites accidental disk-wide walks.
fn has_pattern(entry: &str) -> bool {
    Path::new(entry)
        .file_name()
        .map(|n| {
            let n = n.to_string_lossy();
            n.contains('*') || n.contains('?')
        })
        .unwrap_or(false)
}

/// Files in `pattern`'s parent directory whose name matches it, sorted.
fn expand_pattern(pattern: &Path) -> Vec<String> {
    let dir = match pattern.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let glob = pattern.file_name().unwrap_or_default().to_string_lossy();

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut matched: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter(|e| matches_glob(&glob, &e.file_name().to_string_lossy()))
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    matched.sort();
    matched
}

/// Shell-style match of `name` against `pattern`, supporting `*` (any run of
/// characters) and `?` (exactly one). Case-insensitive, because both target
/// platforms have case-insensitive file systems and a user typing `*.PDF`
/// means the same thing as `*.pdf`.
///
/// Implemented as a small backtracking matcher rather than by pulling in a glob
/// crate: the grammar is two metacharacters wide and this way the behaviour is
/// identical on every platform.
pub fn matches_glob(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();

    // `star`/`mark` remember the last `*` seen and how much of `name` it had
    // consumed, so a dead end can resume by letting it swallow one more char.
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);

    while ni < n.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some(pi);
                mark = ni;
                pi += 1;
            }
            Some('?') => {
                pi += 1;
                ni += 1;
            }
            Some(c) if *c == n[ni] => {
                pi += 1;
                ni += 1;
            }
            _ => match star {
                Some(s) => {
                    pi = s + 1;
                    mark += 1;
                    ni = mark;
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_SIZE_LIMIT: u32 = 4096;

    fn touch(dir: &Path, name: &str) -> String {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, b"%PDF-1.4").unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn fr_008_glob_matcher_handles_the_documented_patterns() {
        assert!(matches_glob("*.pdf", "report.pdf"));
        assert!(matches_glob("*.PDF", "report.pdf"));
        assert!(matches_glob("*", "anything"));
        assert!(matches_glob("report?.pdf", "report1.pdf"));
        assert!(matches_glob("a*b*c", "axxbyyc"));
        assert!(!matches_glob("*.pdf", "report.png"));
        assert!(!matches_glob("report?.pdf", "report12.pdf"));
        assert!(!matches_glob("a*b", "ac"));
        assert!(matches_glob("a*", "a"));
    }

    #[test]
    fn fr_008_expands_a_pattern_in_the_last_component() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "b.pdf");
        touch(dir.path(), "a.pdf");
        touch(dir.path(), "notes.txt");

        let pattern = dir.path().join("*.pdf").to_string_lossy().into_owned();
        let files = expand_inputs(&[pattern], NO_SIZE_LIMIT).unwrap();
        assert_eq!(files.len(), 2, "only the PDFs match");
        assert!(
            files[0].ends_with("a.pdf"),
            "expansion is sorted: {files:?}"
        );
    }

    #[test]
    fn fr_008_expands_a_directory_to_supported_files_only() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "a.pdf");
        touch(dir.path(), "sub/b.png");
        touch(dir.path(), "sub/archive.zip");

        let files =
            expand_inputs(&[dir.path().to_string_lossy().into_owned()], NO_SIZE_LIMIT).unwrap();
        assert_eq!(files.len(), 2, "the .zip is skipped: {files:?}");
    }

    #[test]
    fn fr_008_plain_paths_pass_through_untouched() {
        // Not stat'ed here: a missing file must be reported by name later, with
        // the FileNotFound exit code, not swallowed as "matched nothing".
        // An absolute path in the platform's own form; on Windows a bare
        // `/no/…` is drive-relative and is resolved like any relative entry.
        let missing = if cfg!(windows) {
            r"C:\no\such\file.pdf"
        } else {
            "/no/such/file.pdf"
        };
        let files = expand_inputs(&[missing.to_string()], NO_SIZE_LIMIT).unwrap();
        assert_eq!(files, vec![missing]);
    }

    #[test]
    fn fr_008_relative_entries_resolve_against_the_working_directory() {
        let files = expand_inputs(&["report.pdf".to_string()], NO_SIZE_LIMIT).unwrap();
        assert!(
            Path::new(&files[0]).is_absolute(),
            "a relative entry is made absolute: {files:?}"
        );
        assert!(files[0].ends_with("report.pdf"));
    }

    #[test]
    fn fr_008_a_repeated_entry_prints_once_in_first_appearance_order() {
        let dir = tempfile::tempdir().unwrap();
        let a = touch(dir.path(), "a.pdf");
        let b = touch(dir.path(), "b.pdf");

        // Non-adjacent duplicate: this is what `Vec::dedup` let through.
        let files = expand_inputs(&[a.clone(), b.clone(), a.clone()], NO_SIZE_LIMIT).unwrap();
        assert_eq!(files, vec![a.clone(), b.clone()], "a.pdf printed twice");

        // Adjacent duplicates still collapse, and the order is the order typed —
        // not sorted, because that is the order the pages come out.
        let files = expand_inputs(&[b.clone(), b.clone(), a.clone()], NO_SIZE_LIMIT).unwrap();
        assert_eq!(files, vec![b.clone(), a.clone()]);

        // A pattern that re-matches an explicitly named file does not double it.
        let pattern = dir.path().join("*.pdf").to_string_lossy().into_owned();
        let files = expand_inputs(&[a.clone(), pattern], NO_SIZE_LIMIT).unwrap();
        assert_eq!(files, vec![a, b]);
    }

    #[test]
    fn fr_009_pattern_matching_nothing_is_usage_error() {
        let dir = tempfile::tempdir().unwrap();
        let pattern = dir.path().join("*.pdf").to_string_lossy().into_owned();
        let err = expand_inputs(&[pattern], NO_SIZE_LIMIT).unwrap_err();
        assert_eq!(err.exit, crate::cli::exit::ExitCode::Usage);

        // …and so is an all-blank list.
        assert!(expand_inputs(&["  ".to_string()], NO_SIZE_LIMIT).is_err());
    }
}
