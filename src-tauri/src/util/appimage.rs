//! Starting a system program from inside a Linux AppImage.
//!
//! The AppImage's `AppRun` and the linuxdeploy GTK hook point GTK, GIO and
//! GDK at the libraries bundled in the mounted image (`GTK_PATH`,
//! `GIO_MODULE_DIR`, `GDK_PIXBUF_MODULE_FILE`, `XDG_DATA_DIRS`...), and put
//! the image's folders first on `PATH`. Our own render worker needs exactly
//! that; a program of the host's (LibreOffice, a HEIF decoder, the CUPS
//! tools, a printer settings panel) would load modules built against the
//! bundled libraries, not its own. [`host_command`] starts those with the
//! environment cleaned of the image ([`host_env`]). Outside an AppImage, and
//! on Windows and macOS, it is plain [`Command::new`].

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;

/// A [`Command`] for the host program `program`: inside a Linux AppImage
/// (`APPDIR` set), without the variables that point into the image.
pub fn host_command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    if cfg!(target_os = "linux") {
        if let Some(appdir) = std::env::var_os("APPDIR").filter(|d| !d.is_empty()) {
            apply(&mut cmd, &host_env(Path::new(&appdir), std::env::vars_os()));
        }
    }
    cmd
}

/// Apply the changes [`host_env`] returned to `cmd`.
pub fn apply(cmd: &mut Command, changes: &[(OsString, Option<OsString>)]) {
    for (key, value) in changes {
        match value {
            Some(value) => cmd.env(key, value),
            None => cmd.env_remove(key),
        };
    }
}

/// The environment changes that keep a host program from loading what an
/// AppImage mounted at `appdir` bundles. Pure.
///
/// A list variable (`PATH`, `LD_LIBRARY_PATH`, `XDG_DATA_DIRS`...) loses its
/// entries inside `appdir` and is removed when nothing is left; any other
/// variable whose value points inside `appdir` (`GTK_PATH`,
/// `GDK_PIXBUF_MODULE_FILE`, `GIO_MODULE_DIR`...) is removed, and so are
/// `APPDIR`, `APPIMAGE` and `ARGV0` themselves. `None` means "remove".
pub fn host_env(
    appdir: &Path,
    vars: impl Iterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, Option<OsString>)> {
    let inside = |p: &Path| !appdir.as_os_str().is_empty() && p.starts_with(appdir);
    let mut changes = Vec::new();
    for (key, value) in vars {
        if ["APPDIR", "APPIMAGE", "ARGV0"].iter().any(|k| key == *k) {
            changes.push((key, None));
            continue;
        }
        let entries: Vec<std::path::PathBuf> = std::env::split_paths(&value).collect();
        if !entries.iter().any(|e| inside(e)) {
            continue;
        }
        let kept: Vec<&std::path::PathBuf> = entries.iter().filter(|e| !inside(e)).collect();
        let joined = std::env::join_paths(kept.iter().filter(|e| !e.as_os_str().is_empty()))
            .ok()
            .filter(|v| !v.is_empty());
        changes.push((key, if entries.len() > 1 { joined } else { None }));
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inside an AppImage, a host program loses every variable that points
    /// into the mounted image, and keeps the rest.
    /// Unix only: an AppImage exists on Linux alone, and these are Unix paths
    /// (on Windows `/tmp/...` is not even absolute).
    #[cfg(unix)]
    #[test]
    fn host_programs_do_not_inherit_the_appimage_libraries() {
        let appdir = Path::new("/tmp/.mount_PluryX");
        let vars = [
            ("PATH", "/tmp/.mount_PluryX/usr/bin:/usr/bin:/bin"),
            ("GTK_PATH", "/tmp/.mount_PluryX/usr/lib/gtk-3.0"),
            ("LD_LIBRARY_PATH", "/tmp/.mount_PluryX/usr/lib"),
            ("HOME", "/home/ada"),
            ("APPDIR", "/tmp/.mount_PluryX"),
            ("APPIMAGE", "/home/ada/Pluryprint.AppImage"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let changes = host_env(appdir, vars.into_iter());
        let get = |k: &str| {
            changes
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("PATH"), Some(Some(OsString::from("/usr/bin:/bin"))));
        assert_eq!(get("GTK_PATH"), Some(None));
        assert_eq!(get("LD_LIBRARY_PATH"), Some(None));
        assert_eq!(get("APPDIR"), Some(None));
        assert_eq!(get("APPIMAGE"), Some(None));
        assert_eq!(get("HOME"), None, "untouched");
    }
}
