//! Office, OpenDocument, TXT and RTF → PDF through LibreOffice headless
//! (FR-053, FR-054, FR-057).
//!
//! ```text
//! soffice -env:UserInstallation=file:///<work>/profile --headless --invisible
//!         --nologo --nodefault --nolockcheck --norestore
//!         --convert-to pdf --outdir <work>/out <work>/input.<ext>
//! ```
//!
//! Two details make this safe to run on a real desktop:
//!
//! - **A private profile per conversion.** LibreOffice allows one process per
//!   user profile; without `-env:UserInstallation` a headless convert either
//!   hands the file to the user's open LibreOffice window (and exits without
//!   output) or blocks on its lock. A throwaway profile inside the job's work
//!   directory isolates every conversion, including parallel ones, at the cost
//!   of LibreOffice's first-start time (a few seconds).
//! - **A private copy of the input.** The source is copied to
//!   `input.<ext>` first, so the output name is predictable (`input.pdf`)
//!   whatever Unicode or punctuation the original file name contains, and
//!   LibreOffice never writes a lock file next to the user's document.
//!
//! - **A hardened profile.** Before LibreOffice starts, the private profile
//!   is seeded with [`HARDENING_XCU`] (security finding): a document someone
//!   sent the user must not make the conversion fetch what it links to — a
//!   picture or OLE object at `http://…` (the sender learns the IP and the
//!   time it was printed) or at `\\host\share\…` (Windows hands that host the
//!   user's NTLM credentials over SMB/WebDAV). Links from documents outside
//!   the trusted locations (ours is a temp copy: never trusted) are blocked,
//!   Writer and Calc never update links on load, macros and active content
//!   (OLE, DDE) are off, and LibreOffice's own HTTP(S) traffic is pointed at
//!   a dead proxy on loopback.
//!
//! - **A folder a sandboxed LibreOffice can read** (Linux). A snap or a
//!   Flatpak LibreOffice has a private `/tmp` and cannot see the system temp
//!   folder the job's work directory normally lives in, so for those the work
//!   directory goes where the sandbox always has access: the snap's
//!   `~/snap/<name>/common` or the Flatpak's `~/.var/app/<id>/cache`
//!   ([`confined_work_parent`]).
//!
//! Plain text no longer comes here (FR-057, [`super::text`]); the
//! `--infilter` branch stays for a caller that still sends a `.txt`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::process::run_bounded;
use super::{file_url, WorkDir};
use crate::engine::{EngineError, Failure};

/// Extensions LibreOffice converts for us. Legacy binary Office formats ride
/// the same path at no extra cost.
/// Plain text left this route for [`super::text`] (FR-057).
pub const OFFICE_EXTS: &[&str] = &[
    "docx", "doc", "xlsx", "xls", "pptx", "ppt", "odt", "ods", "odp", "rtf",
];

/// The user-profile settings every conversion starts with (module docs),
/// written as the profile's `user/registrymodifications.xcu`.
///
/// - `Common/Security/Scripting`: `BlockUntrustedRefererLinks` (no linked
///   picture, frame or object loads from a document outside the trusted
///   locations), `DisableMacrosExecution` + `MacroSecurityLevel` 3 (very
///   high), `DisableActiveContent` (no OLE, no DDE).
/// - `Writer/Content/Update/Link` = 2 and `Calc/Content/Update/Link` = 1:
///   "never update links on load" in each application's own numbering
///   (Writer: always 0, on request 1, never 2; Calc: always 0, never 1, on
///   request 2).
/// - `Inet/Settings`: a manual proxy on `127.0.0.1:9` for HTTP and HTTPS
///   with no exceptions, so anything that still asks for a URL fails at once.
pub const HARDENING_XCU: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="BlockUntrustedRefererLinks" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="DisableMacrosExecution" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="DisableActiveContent" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Writer/Content/Update"><prop oor:name="Link" oor:op="fuse"><value>2</value></prop></item>
<item oor:path="/org.openoffice.Office.Calc/Content/Update"><prop oor:name="Link" oor:op="fuse"><value>1</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetProxyType" oor:op="fuse"><value>2</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetHTTPProxyName" oor:op="fuse"><value>127.0.0.1</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetHTTPProxyPort" oor:op="fuse"><value>9</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetHTTPSProxyName" oor:op="fuse"><value>127.0.0.1</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetHTTPSProxyPort" oor:op="fuse"><value>9</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetNoProxy" oor:op="fuse"><value></value></prop></item>
</oor:items>
"#;

/// Seed the private profile at `profile_dir` with [`HARDENING_XCU`].
pub fn harden_profile(profile_dir: &Path) -> std::io::Result<()> {
    let user = profile_dir.join("user");
    std::fs::create_dir_all(&user)?;
    std::fs::write(user.join("registrymodifications.xcu"), HARDENING_XCU)
}

/// Where the work directory of a conversion by `soffice` must live, when that
/// LibreOffice runs in a sandbox that cannot read the system temp folder.
/// `None` for an ordinary install. Pure.
///
/// - A **snap** is started through its launcher in `/snap/bin/<snap>[.<app>]`
///   and can always read and write `~/snap/<snap>/common`.
/// - A **Flatpak** is started through the launcher it exports (named after
///   its app id, [`crate::engine::deps::FLATPAK_LIBREOFFICE`]) and always
///   has its own `~/.var/app/<app id>` at the same path inside the sandbox.
pub fn confined_work_parent(soffice: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let home = home?;
    let name = soffice.file_name()?.to_str()?;
    if soffice.starts_with("/snap/bin") {
        let snap = name.split('.').next().filter(|s| !s.is_empty())?;
        return Some(home.join("snap").join(snap).join("common"));
    }
    if name == crate::engine::deps::FLATPAK_LIBREOFFICE {
        return Some(home.join(".var").join("app").join(name).join("cache"));
    }
    None
}

/// The argument vector for one conversion (everything after the program).
/// Pure, so the flags are pinned by a unit test rather than by reading code.
pub fn soffice_args(profile_dir: &Path, out_dir: &Path, input: &Path, ext: &str) -> Vec<String> {
    let mut args = vec![
        format!("-env:UserInstallation={}", file_url(profile_dir)),
        "--headless".to_string(),
        "--invisible".to_string(),
        "--nologo".to_string(),
        "--nodefault".to_string(),
        "--nolockcheck".to_string(),
        "--norestore".to_string(),
    ];
    if ext == "txt" {
        // Filter options: charset, line ending, font (default), language.
        args.push("--infilter=Text (encoded):UTF8,LF,,,".to_string());
    }
    args.extend([
        "--convert-to".to_string(),
        "pdf".to_string(),
        "--outdir".to_string(),
        out_dir.to_string_lossy().into_owned(),
        input.to_string_lossy().into_owned(),
    ]);
    args
}

/// Convert `input` with the LibreOffice at `soffice`, writing the PDF to
/// `output`. `work` is the job's scratch directory (deleted by the caller).
pub fn convert_with(
    soffice: &Path,
    input: &Path,
    output: &Path,
    work: &WorkDir,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
) -> Result<(), EngineError> {
    let ext = crate::engine::extension_of(input);
    let staged = work.path().join(format!("input.{ext}"));
    std::fs::copy(input, &staged).map_err(|e| super::cannot_read(input, "read", e))?;
    let profile = work.path().join("profile");
    let out_dir = work.path().join("out");
    std::fs::create_dir_all(&out_dir)
        .and_then(|()| harden_profile(&profile))
        .map_err(|e| {
            EngineError::ConversionFailed(Failure::new(
                "workdir_failed",
                format!("cannot prepare the conversion folder: {e}"),
            ))
        })?;

    let mut cmd = Command::new(soffice);
    cmd.args(soffice_args(&profile, &out_dir, &staged, &ext))
        .current_dir(work.path());
    run_bounded(
        cmd,
        "LibreOffice",
        cancel,
        timeout,
        &work.path().join("soffice.log"),
    )?;

    // soffice exits 0 even when it could not load the document ("Error:
    // source file could not be loaded"), so the output file is the real
    // verdict.
    let produced: PathBuf = out_dir.join("input.pdf");
    super::take_output(
        &produced,
        output,
        "LibreOffice",
        &work.path().join("soffice.log"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FR-053 on Linux: a sandboxed LibreOffice converts from a folder its
    /// sandbox can read; an ordinary install keeps the system temp folder.
    #[test]
    fn fr_053_sandboxed_libreoffice_converts_inside_its_own_folder() {
        let home = Path::new("/home/ada");
        assert_eq!(
            confined_work_parent(Path::new("/snap/bin/libreoffice"), Some(home)),
            Some(PathBuf::from("/home/ada/snap/libreoffice/common"))
        );
        assert_eq!(
            confined_work_parent(Path::new("/snap/bin/libreoffice.soffice"), Some(home)),
            Some(PathBuf::from("/home/ada/snap/libreoffice/common"))
        );
        for flatpak in [
            "/var/lib/flatpak/exports/bin/org.libreoffice.LibreOffice",
            "/home/ada/.local/share/flatpak/exports/bin/org.libreoffice.LibreOffice",
        ] {
            assert_eq!(
                confined_work_parent(Path::new(flatpak), Some(home)),
                Some(PathBuf::from(
                    "/home/ada/.var/app/org.libreoffice.LibreOffice/cache"
                ))
            );
        }
        for plain in [
            "/usr/bin/soffice",
            "/usr/lib/libreoffice/program/soffice",
            "/opt/libreoffice25.2/program/soffice",
            "/Applications/LibreOffice.app/Contents/MacOS/soffice",
        ] {
            assert_eq!(confined_work_parent(Path::new(plain), Some(home)), None);
        }
        assert_eq!(
            confined_work_parent(Path::new("/snap/bin/libreoffice"), None),
            None
        );
    }

    #[test]
    fn fr_053_soffice_args_use_a_private_profile_and_headless_convert() {
        // Platform-absolute paths: `/w` is not absolute on Windows.
        let (w, url) = if cfg!(windows) {
            (r"C:\w", "file:///C:/w/profile")
        } else {
            ("/w", "file:///w/profile")
        };
        let w = Path::new(w);
        let args = soffice_args(
            &w.join("profile"),
            &w.join("out"),
            &w.join("input.docx"),
            "docx",
        );
        assert_eq!(args[0], format!("-env:UserInstallation={url}"));
        assert!(args.contains(&"--headless".to_string()));
        assert!(args.contains(&"--norestore".to_string()));
        let tail: Vec<&str> = args
            .iter()
            .rev()
            .take(5)
            .rev()
            .map(String::as_str)
            .collect();
        let out = w.join("out").to_string_lossy().into_owned();
        let input = w.join("input.docx").to_string_lossy().into_owned();
        assert_eq!(tail, ["--convert-to", "pdf", "--outdir", &out, &input]);
        assert!(!args.iter().any(|a| a.starts_with("--infilter")));
    }

    /// Security finding: the private profile blocks linked resources,
    /// macros and active content, and points LibreOffice's HTTP(S) at a dead
    /// proxy, before LibreOffice ever starts.
    #[test]
    fn fr_053_the_private_profile_is_hardened_before_launch() {
        let dir = tempfile::tempdir().unwrap();
        harden_profile(&dir.path().join("profile")).unwrap();
        let xcu =
            std::fs::read_to_string(dir.path().join("profile/user/registrymodifications.xcu"))
                .unwrap();
        for (path, name, value) in [
            (
                "/org.openoffice.Office.Common/Security/Scripting",
                "BlockUntrustedRefererLinks",
                "true",
            ),
            (
                "/org.openoffice.Office.Common/Security/Scripting",
                "DisableMacrosExecution",
                "true",
            ),
            (
                "/org.openoffice.Office.Common/Security/Scripting",
                "DisableActiveContent",
                "true",
            ),
            ("/org.openoffice.Office.Writer/Content/Update", "Link", "2"),
            ("/org.openoffice.Office.Calc/Content/Update", "Link", "1"),
            ("/org.openoffice.Inet/Settings", "ooInetProxyType", "2"),
            ("/org.openoffice.Inet/Settings", "ooInetHTTPProxyPort", "9"),
            ("/org.openoffice.Inet/Settings", "ooInetHTTPSProxyPort", "9"),
        ] {
            let item = format!(
                "<item oor:path=\"{path}\"><prop oor:name=\"{name}\" oor:op=\"fuse\"><value>{value}</value></prop></item>"
            );
            assert!(xcu.contains(&item), "{item}");
        }
        // Well-formed enough for LibreOffice: one root, every item closed.
        assert_eq!(
            xcu.matches("<item ").count(),
            xcu.matches("</item>").count()
        );
        assert!(xcu.trim_end().ends_with("</oor:items>"));
    }

    #[test]
    fn fr_057_txt_is_imported_as_utf8() {
        let args = soffice_args(
            Path::new("/w/p"),
            Path::new("/w/o"),
            Path::new("/w/input.txt"),
            "txt",
        );
        assert!(args.contains(&"--infilter=Text (encoded):UTF8,LF,,,".to_string()));
    }
}
