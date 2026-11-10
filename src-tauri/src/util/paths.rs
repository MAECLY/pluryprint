//! Filesystem path resolution.
//!
//! Centralizes every hardcoded directory name so the rest of the codebase
//! never spells out "Pluryprint" or "Maecly". The layout is whatever
//! `directories::ProjectDirs::from("com", "Maecly", "Pluryprint")` gives, and
//! the shell extension resolves the same way, so it must not drift:
//!
//! | Dir    | Windows                                  | macOS                                            |
//! | ------ | ---------------------------------------- | ------------------------------------------------ |
//! | config | `%APPDATA%\Maecly\Pluryprint\config\`     | `~/Library/Application Support/com.Maecly.Pluryprint/` |
//! | data   | `%LOCALAPPDATA%\Maecly\Pluryprint\data\`  | `~/Library/Application Support/com.Maecly.Pluryprint/` |
//! | cache  | `%LOCALAPPDATA%\Maecly\Pluryprint\cache\` | `~/Library/Caches/com.Maecly.Pluryprint/`        |
//! | logs   | `<data>\logs\`                            | `~/Library/Logs/Pluryprint/`                     |
//!
//! On Linux the same call follows the XDG base directories (each honours its
//! `XDG_*_HOME` variable when set):
//!
//! | Dir    | Linux                                      |
//! | ------ | ------------------------------------------ |
//! | config | `~/.config/pluryprint/`                    |
//! | data   | `~/.local/share/pluryprint/`               |
//! | cache  | `~/.cache/pluryprint/`                     |
//! | logs   | `~/.local/share/pluryprint/logs/`          |
//!
//! See `docs/04-architecture/data-model.md` §1 for the full table.
//!
//! It also owns [`canonical_input`], the single gate every path that arrives
//! from outside the process (IPC, drag-and-drop, CLI) goes through (NFR-025).

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::util::error::IpcError;

const QUALIFIER: &str = "com";
const ORG: &str = "Maecly";
const APP: &str = "Pluryprint";

fn dirs() -> ProjectDirs {
    ProjectDirs::from(QUALIFIER, ORG, APP).expect("home directory must be resolvable")
}

/// Where `config.toml` and `profiles.toml` live.
/// Windows: `%APPDATA%\Maecly\Pluryprint\config\`
/// macOS:   `~/Library/Application Support/com.Maecly.Pluryprint/`
/// Linux:   `~/.config/pluryprint/` (`$XDG_CONFIG_HOME`)
pub fn config_dir() -> PathBuf {
    dirs().config_dir().to_path_buf()
}

/// Where `queue.json` and other live state live.
/// Windows: `%LOCALAPPDATA%\Maecly\Pluryprint\data\`
/// macOS:   `~/Library/Application Support/com.Maecly.Pluryprint/`
/// Linux:   `~/.local/share/pluryprint/` (`$XDG_DATA_HOME`)
pub fn data_dir() -> PathBuf {
    dirs().data_local_dir().to_path_buf()
}

/// Where converted PDFs live (`conversions/`).
/// Windows: `%LOCALAPPDATA%\Maecly\Pluryprint\cache\`
/// macOS:   `~/Library/Caches/com.Maecly.Pluryprint/`
/// Linux:   `~/.cache/pluryprint/` (`$XDG_CACHE_HOME`)
pub fn cache_dir() -> PathBuf {
    dirs().cache_dir().to_path_buf()
}

/// Where rotating JSONL logs are written.
/// Windows: `%LOCALAPPDATA%\Maecly\Pluryprint\data\logs\`
/// macOS:   `~/Library/Logs/Pluryprint/`
/// Linux:   `~/.local/share/pluryprint/logs/`
pub fn log_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = directories::UserDirs::new() {
            return home.home_dir().join("Library/Logs").join(APP);
        }
        data_dir().join("logs")
    }
    #[cfg(not(target_os = "macos"))]
    {
        data_dir().join("logs")
    }
}

/// Ensure a directory exists; create it (and parents) if missing.
pub fn ensure_dir(p: &Path) -> std::io::Result<()> {
    if !p.exists() {
        std::fs::create_dir_all(p)?;
    }
    Ok(())
}

/// Resolve a path that arrived from outside the process (NFR-025).
///
/// Everything the frontend, a drop or the CLI hands us is run through here
/// before it is stored or printed:
///
/// - a **relative** path is refused, never resolved: it would silently land on
///   the process' working directory, which is wherever the OS happened to
///   launch the app from (the Explorer window, `/`, the shell extension's cwd)
///   and is never what the user pointed at;
/// - `.`/`..` segments and symlinks are resolved, so the same file always gets
///   the same identity in the queue no matter how it was named;
/// - a path that does not exist is refused up front, rather than failing later
///   inside a converter with a worse message.
///
/// Symlinks resolving to their target is deliberate: a link to a real document
/// prints that document. The caller keeps the original string for display when
/// it needs to.
pub fn canonical_input(raw: &str) -> Result<PathBuf, IpcError> {
    use crate::util::error::IpcErrorCode;
    if raw.trim().is_empty() {
        return Err(IpcError::localized(
            IpcErrorCode::InvalidRequest,
            "path_empty",
            "empty path",
        ));
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(IpcError::localized(
            IpcErrorCode::InvalidRequest,
            "path_not_absolute",
            format!("{raw} is not an absolute path"),
        )
        .with_param("path", raw));
    }
    match path.canonicalize() {
        Ok(p) => Ok(simplified(p)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(IpcError::FileNotFound(raw.to_string()))
        }
        Err(e) => Err(IpcError::localized(
            IpcErrorCode::Internal,
            "path_unresolvable",
            format!("cannot resolve {raw}: {e}"),
        )
        .with_param("path", raw)),
    }
}

/// Windows: `canonicalize` returns the verbatim `\\?\C:\…` form, which the
/// external tools we shell out to (SumatraPDF, the shell) and the UI both
/// dislike, so drive-letter paths are put back in their ordinary form. UNC
/// verbatim paths (`\\?\UNC\…`) are left alone — there the prefix carries
/// meaning.
#[cfg(windows)]
fn simplified(p: PathBuf) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest)
            if rest.len() >= 2
                && rest.as_bytes()[0].is_ascii_alphabetic()
                && rest.as_bytes()[1] == b':' =>
        {
            PathBuf::from(rest)
        }
        _ => p,
    }
}

/// Non-Windows: `canonicalize` already returns a plain absolute path.
#[cfg(not(windows))]
fn simplified(p: PathBuf) -> PathBuf {
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirs_are_resolvable() {
        let c = config_dir();
        let d = data_dir();
        let l = log_dir();
        assert!(c.is_absolute(), "config_dir not absolute: {c:?}");
        assert!(d.is_absolute(), "data_dir not absolute: {d:?}");
        assert!(l.is_absolute(), "log_dir not absolute: {l:?}");
    }

    #[test]
    fn nfr_025_relative_paths_are_refused() {
        use crate::util::error::IpcErrorCode;
        let err = canonical_input("docs/report.pdf").unwrap_err();
        assert_eq!(err.code(), IpcErrorCode::InvalidRequest);
        // FR-092: a reason the UI translates, naming the path it refused.
        assert_eq!(err.reason(), Some("path_not_absolute"));
        assert!(
            err.message().contains("absolute"),
            "expected an absolute-path rejection, got {err:?}"
        );
        let blank = canonical_input("   ").unwrap_err();
        assert_eq!(blank.code(), IpcErrorCode::InvalidRequest);
        assert_eq!(blank.reason(), Some("path_empty"));
    }

    #[test]
    fn nfr_025_missing_paths_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.pdf");
        assert!(matches!(
            canonical_input(&missing.to_string_lossy()).unwrap_err(),
            IpcError::FileNotFound(_)
        ));
    }

    #[test]
    fn nfr_025_dot_segments_collapse_to_one_identity() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let file = sub.join("a.pdf");
        std::fs::write(&file, b"%PDF-1.4\n").unwrap();

        let direct = canonical_input(&file.to_string_lossy()).unwrap();
        let winding = canonical_input(&sub.join("..").join("sub/a.pdf").to_string_lossy()).unwrap();
        assert_eq!(direct, winding);
        assert!(direct.is_absolute());
    }

    /// FR-100 / FR-113: the locations the requirements document are the ones
    /// `ProjectDirs` really resolves to on this OS. Spelled through the same
    /// constants, so the assertion is about the *shape*, not the brand.
    #[test]
    fn fr_100_fr_113_dirs_have_the_documented_shape() {
        #[cfg(target_os = "macos")]
        {
            let bundle = format!("Library/Application Support/{QUALIFIER}.{ORG}.{APP}");
            assert!(config_dir().ends_with(&bundle), "{:?}", config_dir());
            assert!(data_dir().ends_with(&bundle), "{:?}", data_dir());
            assert!(
                cache_dir().ends_with(format!("Library/Caches/{QUALIFIER}.{ORG}.{APP}")),
                "{:?}",
                cache_dir()
            );
            assert!(
                log_dir().ends_with(format!("Library/Logs/{APP}")),
                "{:?}",
                log_dir()
            );
        }
        #[cfg(windows)]
        {
            let roaming = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap());
            let local = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
            assert_eq!(config_dir(), roaming.join(ORG).join(APP).join("config"));
            assert_eq!(data_dir(), local.join(ORG).join(APP).join("data"));
            assert_eq!(cache_dir(), local.join(ORG).join(APP).join("cache"));
            assert_eq!(
                log_dir(),
                local.join(ORG).join(APP).join("data").join("logs")
            );
        }
    }

    /// The user's data location, byte for byte, as `directories` 5 resolved it
    /// in 0.0.1. Spelled out as literals on purpose, not through the constants
    /// or `ProjectDirs`: installed copies, the Explorer shell extension, the
    /// NSIS uninstall hooks and `util::legacy` all know these exact strings, so
    /// a `directories` upgrade (or a constant edit) that moves any of them must
    /// fail here rather than strand the user's settings and queue.
    #[test]
    fn data_location_is_pinned_byte_for_byte() {
        #[cfg(target_os = "macos")]
        {
            let home = std::env::var("HOME").expect("HOME is set");
            let s = |p: PathBuf| p.to_string_lossy().into_owned();
            assert_eq!(
                s(config_dir()),
                format!("{home}/Library/Application Support/com.Maecly.Pluryprint")
            );
            assert_eq!(
                s(data_dir()),
                format!("{home}/Library/Application Support/com.Maecly.Pluryprint")
            );
            assert_eq!(
                s(cache_dir()),
                format!("{home}/Library/Caches/com.Maecly.Pluryprint")
            );
            assert_eq!(s(log_dir()), format!("{home}/Library/Logs/Pluryprint"));
        }
        #[cfg(windows)]
        {
            let roaming = std::env::var("APPDATA").expect("APPDATA is set");
            let local = std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA is set");
            let s = |p: PathBuf| p.to_string_lossy().into_owned();
            assert_eq!(
                s(config_dir()),
                format!(r"{roaming}\Maecly\Pluryprint\config")
            );
            assert_eq!(s(data_dir()), format!(r"{local}\Maecly\Pluryprint\data"));
            assert_eq!(s(cache_dir()), format!(r"{local}\Maecly\Pluryprint\cache"));
            assert_eq!(
                s(log_dir()),
                format!(r"{local}\Maecly\Pluryprint\data\logs")
            );
        }
        #[cfg(target_os = "linux")]
        {
            let home = std::env::var("HOME").expect("HOME is set");
            let s = |p: PathBuf| p.to_string_lossy().into_owned();
            // An absolute XDG_*_HOME replaces the default, per the spec.
            let xdg = |var: &str, default: &str| {
                std::env::var(var)
                    .ok()
                    .filter(|v| v.starts_with('/'))
                    .unwrap_or_else(|| format!("{home}/{default}"))
            };
            let data = format!("{}/pluryprint", xdg("XDG_DATA_HOME", ".local/share"));
            assert_eq!(
                s(config_dir()),
                format!("{}/pluryprint", xdg("XDG_CONFIG_HOME", ".config"))
            );
            assert_eq!(s(data_dir()), data);
            assert_eq!(
                s(cache_dir()),
                format!("{}/pluryprint", xdg("XDG_CACHE_HOME", ".cache"))
            );
            assert_eq!(s(log_dir()), format!("{data}/logs"));
        }
    }

    #[test]
    fn dirs_have_app_segment() {
        // All resolved dirs should contain the literal "Pluryprint" segment somewhere.
        for p in [config_dir(), data_dir(), cache_dir()] {
            let s = p.to_string_lossy().to_lowercase();
            assert!(
                s.contains("pluryprint"),
                "expected Pluryprint segment in {p:?}"
            );
        }
    }
}
