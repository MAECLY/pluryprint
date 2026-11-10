//! Signed update download + install (FR-082, FR-083, FR-084 "install now",
//! FR-087). See ADR-0008.
//!
//! The heavy lifting is `tauri-plugin-updater`: it reads the release manifest
//! (`latest.json`), downloads the artifact for this platform, verifies its
//! minisign signature against the public key embedded in `tauri.conf.json`
//! (`plugins.updater.pubkey`) **before** anything touches the disk, and runs
//! the platform installer — the per-user NSIS setup on Windows (no admin,
//! FR-083), an in-place `.app` swap on macOS, and on Linux either an in-place
//! swap of the AppImage file or `pkexec dpkg -i` for the `.deb` (the user is
//! asked for the administrator password, as for any package).
//!
//! This module owns the policy around it, as pure functions with co-located
//! tests, so the GUI command ([`crate::ipc::update::install_update`]) and the
//! CLI (`pluryprint update install`) apply exactly the same rules:
//!
//! - [`resolve_endpoint`] — the manifest URL of the user's channel
//!   ([`default_endpoint`], FR-085), overridable through [`ENDPOINT_ENV`] for
//!   local end-to-end testing.
//! - [`is_major_bump`] — FR-087: a major version jump is never installed
//!   without an explicit, separate opt-in.
//! - [`check_installable_location`] / [`check_linux_install`] — refuse to
//!   "update" a binary that is not an installed app (a dev build in
//!   `target/`), which on macOS would replace the directory the binary
//!   happens to live in, and on Linux would be overwritten as if it were an
//!   AppImage. [`check_running_copy`] applies the right one to this process.
//! - [`classify_error`] — a tampered or unsigned artifact surfaces as
//!   [`IpcError::UpdateSignatureInvalid`] with a message that says nothing was
//!   installed, never as a generic failure.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use specta::Type;
use tauri::Url;
use tauri_plugin_updater::UpdaterExt;

use crate::settings::UpdateChannel;
use crate::util::error::{IpcError, IpcErrorCode};

/// The stable channel's manifest. `releases/latest/download` always
/// redirects to the newest published release that is neither a draft nor a
/// prerelease, so the URL never changes (FR-085).
pub const STABLE_ENDPOINT: &str =
    "https://github.com/MAECLY/pluryprint/releases/latest/download/latest.json";

/// The tag of the fixed prerelease that carries the beta channel's manifest.
/// `.github/workflows/release.yml` overwrites its `latest.json` on **every**
/// release, stable ones included, so a beta user is also offered a stable
/// release that is newer than their beta (FR-086).
pub const BETA_CHANNEL_TAG: &str = "channel-beta";

/// The beta channel's manifest (FR-085).
pub const BETA_ENDPOINT: &str =
    "https://github.com/MAECLY/pluryprint/releases/download/channel-beta/latest.json";

/// The endpoint `tauri.conf.json` names, which is the stable channel's. The
/// app always passes its own endpoint list to the updater
/// ([`find_update_with`]), so this only matters to a tool that reads the
/// config.
pub const DEFAULT_ENDPOINT: &str = STABLE_ENDPOINT;

/// The manifest of `channel` (FR-085). No nightly builds are published, so
/// the nightly channel follows the beta manifest: it gets every prerelease
/// and every stable release, never less than beta. Pure.
pub fn default_endpoint(channel: UpdateChannel) -> &'static str {
    match channel {
        UpdateChannel::Stable => STABLE_ENDPOINT,
        UpdateChannel::Beta | UpdateChannel::Nightly => BETA_ENDPOINT,
    }
}

/// Environment variable that replaces the channel's endpoint, for testing an
/// update end to end against a local server. Release builds still require
/// `https` (the plugin refuses plain `http` outside debug builds). The
/// deprecated `MAECLYPRINT_UPDATE_ENDPOINT` is still read, with a warning.
pub const ENDPOINT_ENV: &str = "PLURYPRINT_UPDATE_ENDPOINT";

/// The manifest URL: `override_value` when it is set and non-blank, the
/// `channel`'s manifest otherwise. Pure.
pub fn resolve_endpoint(
    override_value: Option<&str>,
    channel: UpdateChannel,
) -> Result<Url, String> {
    let raw = override_value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(default_endpoint(channel));
    let url = Url::parse(raw).map_err(|e| format!("invalid update endpoint '{raw}': {e}"))?;
    match url.scheme() {
        "https" | "http" => Ok(url),
        other => Err(format!(
            "invalid update endpoint '{raw}': unsupported scheme '{other}'"
        )),
    }
}

/// The value of [`ENDPOINT_ENV`] (or its deprecated name), when it is set and
/// non-blank.
pub fn endpoint_override() -> Option<String> {
    crate::util::legacy::env_var(ENDPOINT_ENV).filter(|v| !v.trim().is_empty())
}

/// [`resolve_endpoint`] against the real environment.
pub fn endpoint(channel: UpdateChannel) -> Result<Url, String> {
    resolve_endpoint(endpoint_override().as_deref(), channel)
}

/// FR-087: whether going from `current` to `candidate` crosses a **major**
/// version (`1.x` → `2.x`). Pure.
///
/// Such an update is never installed on the strength of a single "install"
/// action: the GUI asks for a second, explicit confirmation and the CLI
/// requires `--yes` or an interactive "yes". An unparseable version on either
/// side counts as a major bump — when in doubt, ask.
pub fn is_major_bump(current: &str, candidate: &str) -> bool {
    let parse = |s: &str| semver::Version::parse(s.trim().trim_start_matches(['v', 'V']));
    match (parse(current), parse(candidate)) {
        (Ok(cur), Ok(new)) => new.major > cur.major,
        _ => true,
    }
}

/// FR-087's gate, pure: may `candidate` be installed over `current`?
///
/// - A major jump needs `allow_major`.
/// - A major jump also has to be the version the user confirmed. The About
///   screen asks about the release the GitHub API named, then the install
///   reads the update manifest again; if that now names another version (a
///   newer major published in between, or a manifest that differs from the
///   API), the confirmation was not for it. `confirmed` is that version, or
///   `None` where the opt-in is blanket (the CLI's `--yes`).
///
/// A minor or patch update needs no confirmation, so its version is not
/// compared: a manifest that runs ahead of the API must not block it.
pub fn check_confirmation(
    current: &str,
    candidate: &str,
    allow_major: bool,
    confirmed: Option<&str>,
) -> Result<(), IpcError> {
    if !is_major_bump(current, candidate) {
        return Ok(());
    }
    if !allow_major {
        return Err(IpcError::localized(
            IpcErrorCode::InvalidRequest,
            "major_update_unconfirmed",
            format!(
                "{candidate} is a major update from {current}; it is only installed after you confirm it explicitly"
            ),
        )
        .with_param("version", candidate)
        .with_param("current", current));
    }
    let bare = |v: &str| v.trim().trim_start_matches(['v', 'V']).to_string();
    match confirmed {
        Some(confirmed) if bare(confirmed) != bare(candidate) => Err(IpcError::localized(
            IpcErrorCode::InvalidRequest,
            "major_update_changed",
            format!(
                "you confirmed {confirmed}, but the update available now is {candidate}; check for updates again and confirm that version"
            ),
        )
        .with_param("version", candidate)
        .with_param("confirmed", confirmed)),
        _ => Ok(()),
    }
}

/// Refuse to install over something that is not an installed copy of the app.
/// Pure (`os` is `std::env::consts::OS`).
///
/// On macOS the updater replaces the `.app` bundle the executable lives in; a
/// binary that is *not* inside `<Name>.app/Contents/MacOS/` (a `cargo run`
/// build) would have its whole parent directory swapped out instead. Windows
/// runs the per-user installer, which installs to its own location, so any
/// executable can hand over to it.
pub fn check_installable_location(os: &str, exe: &Path) -> Result<(), String> {
    if os != "macos" {
        return Ok(());
    }
    let in_bundle = exe
        .parent()
        .filter(|macos| macos.file_name().is_some_and(|n| n == "MacOS"))
        .and_then(Path::parent)
        .filter(|contents| contents.file_name().is_some_and(|n| n == "Contents"))
        .and_then(Path::parent)
        .and_then(Path::extension)
        .is_some_and(|ext| ext == "app");
    if in_bundle {
        Ok(())
    } else {
        Err(format!(
            "this copy of Pluryprint ({}) is not an installed app bundle, so it cannot \
             update itself; download the release from the releases page instead",
            exe.display()
        ))
    }
}

/// How the running copy was installed on Linux, which is what decides how the
/// updater replaces it (the bundler stamps it into the main executable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxPackage {
    /// A `.AppImage` file: the updater writes the new one over it, in place.
    AppImage,
    /// The `.deb` package: the updater installs the new one with
    /// `pkexec dpkg -i`.
    Deb,
    /// An `.rpm` package (`pkexec rpm -U`). Not published, but a build of
    /// ours packaged that way updates like the `.deb`.
    Rpm,
    /// Not stamped: a `cargo` build, a binary copied out of a package, or
    /// `pluryprint-cli` (only the main executable is stamped).
    Unknown,
}

/// Refuse to update a Linux copy the updater would damage or miss. Pure.
///
/// The updater treats anything that is not a package as an AppImage and
/// writes the update over it. That is right only for a real AppImage, named
/// by the `APPIMAGE` variable its runtime sets (the executable itself runs
/// from a temporary mount). A package install must be the file the package
/// put there (`packaged`, which asks `dpkg`/`rpm`), or the package manager
/// would update a different copy than the one running.
pub fn check_linux_install(
    package: LinuxPackage,
    exe: &Path,
    appimage: Option<&Path>,
    packaged: &dyn Fn(&Path) -> bool,
) -> Result<(), String> {
    match package {
        LinuxPackage::AppImage => match appimage {
            Some(file) if file.is_absolute() => Ok(()),
            _ => Err(
                "this copy of Pluryprint was started from inside its AppImage, not from the \
                 .AppImage file, so it cannot update itself; start the .AppImage file, or \
                 download the release from the releases page instead"
                    .to_string(),
            ),
        },
        LinuxPackage::Deb | LinuxPackage::Rpm => {
            if packaged(exe) {
                Ok(())
            } else {
                let kind = if package == LinuxPackage::Deb {
                    ".deb"
                } else {
                    ".rpm"
                };
                Err(format!(
                    "this copy of Pluryprint ({}) is not the one the {kind} package installed, \
                     so it cannot update itself; run the installed Pluryprint, or download \
                     the release from the releases page instead",
                    exe.display()
                ))
            }
        }
        LinuxPackage::Unknown => Err(format!(
            "this copy of Pluryprint ({}) is neither an AppImage nor the program a package \
             installed, so it cannot update itself (with the .deb, update from the app or \
             with `pluryprint update install`); download the release from the releases \
             page instead",
            exe.display()
        )),
    }
}

/// [`check_installable_location`] or, on Linux, [`check_linux_install`],
/// for this process.
pub fn check_running_copy(exe: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use tauri::utils::config::BundleType;
        let package = match tauri::utils::platform::bundle_type() {
            Some(BundleType::AppImage) => LinuxPackage::AppImage,
            Some(BundleType::Deb) => LinuxPackage::Deb,
            Some(BundleType::Rpm) => LinuxPackage::Rpm,
            _ => LinuxPackage::Unknown,
        };
        let appimage = std::env::var_os("APPIMAGE").map(std::path::PathBuf::from);
        check_linux_install(package, exe, appimage.as_deref(), &|file| {
            package_owns(package, file)
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        check_installable_location(std::env::consts::OS, exe)
    }
}

/// Whether the package manager of `package` lists `file` as installed by a
/// package (`dpkg-query -S`, `rpm -qf`).
#[cfg(target_os = "linux")]
fn package_owns(package: LinuxPackage, file: &Path) -> bool {
    let (program, args): (&str, &[&str]) = match package {
        LinuxPackage::Deb => ("dpkg-query", &["-S"]),
        LinuxPackage::Rpm => ("rpm", &["-qf"]),
        _ => return false,
    };
    std::process::Command::new(program)
        .args(args)
        .arg(file)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The user-facing message for an artifact that failed verification.
pub const SIGNATURE_REJECTED: &str =
    "the downloaded update failed its signature check and was NOT installed. \
     It may have been tampered with or corrupted in transit; nothing on this \
     computer was changed. Try again later, or download the release from the \
     releases page";

/// Map an updater failure onto the IPC vocabulary. Pure.
///
/// Every way the signature can be wrong — bad signature, wrong key, a
/// signature that is not even valid base64, a version that does not match
/// what was signed — is the same thing to the user: the file is not trusted
/// and was not installed. That gets its own code so the UI can say so plainly.
pub fn classify_error(err: &tauri_plugin_updater::Error) -> IpcError {
    use tauri_plugin_updater::Error as E;
    match err {
        E::Minisign(_)
        | E::Base64(_)
        | E::SignatureUtf8(_)
        | E::SignedVersionMismatch { .. }
        | E::MissingSignedVersion => IpcError::localized(
            IpcErrorCode::UpdateSignatureInvalid,
            "signature_rejected",
            format!("{SIGNATURE_REJECTED} ({err})"),
        ),
        E::Reqwest(_) | E::Network(_) => IpcError::localized(
            IpcErrorCode::UpdateInstallFailed,
            "download_failed",
            format!("could not download the update — check your connection ({err})"),
        ),
        E::ReleaseNotFound | E::Serialization(_) | E::TargetNotFound(_) | E::TargetsNotFound(_) => {
            IpcError::localized(
                IpcErrorCode::UpdateInstallFailed,
                "no_platform_update",
                format!("the release does not offer an update for this platform yet ({err})"),
            )
        }
        // The manifest names a file of the wrong kind for how this copy was
        // installed (an AppImage for a .deb, or the reverse): a release
        // problem, and nothing was touched.
        E::InvalidUpdaterFormat | E::BinaryNotFoundInArchive => IpcError::localized(
            IpcErrorCode::UpdateInstallFailed,
            "wrong_package_format",
            format!(
                "the update published for this platform is not the kind of package this \
                 copy was installed from, so nothing was installed; download the release \
                 from the releases page instead ({err})"
            ),
        ),
        // AppImage: no folder on the AppImage's file system could be written
        // to, so the file could not be swapped.
        E::TempDirNotOnSameMountPoint => IpcError::localized(
            IpcErrorCode::UpdateInstallFailed,
            "appimage_not_writable",
            format!(
                "the update could not replace the AppImage because its folder cannot be \
                 written to; move the .AppImage to a folder you own (such as \
                 ~/Applications) and try again ({err})"
            ),
        ),
        // .deb: the administrator password prompt was dismissed or failed.
        E::AuthenticationFailed => IpcError::localized(
            IpcErrorCode::UpdateInstallFailed,
            "auth_cancelled",
            format!(
                "the update was not installed: the administrator password prompt was \
                 cancelled or failed, and nothing was changed ({err})"
            ),
        ),
        E::DebInstallFailed | E::PackageInstallFailed => IpcError::localized(
            IpcErrorCode::UpdateInstallFailed,
            "package_install_failed",
            format!(
                "the system package manager could not install the update; install the \
                 package from the releases page instead ({err})"
            ),
        ),
        E::PreviousAppNotRestored(path) => IpcError::localized(
            IpcErrorCode::UpdateInstallFailed,
            "previous_app_not_restored",
            format!("{err}; move it back, or reinstall from the releases page"),
        )
        .with_param("path", path.display()),
        E::Io(_) | E::TempDirNotFound => IpcError::localized(
            IpcErrorCode::UpdateInstallFailed,
            "install_failed",
            format!("the update could not be installed ({err})"),
        ),
        other => IpcError::UpdateInstallFailed(other.to_string()),
    }
}

/// What an install attempt did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct UpdateInstallResult {
    /// The version that was installed, or `None` when there was nothing newer.
    pub installed: Option<String>,
    /// The new version runs only after a restart (macOS, Linux). On Windows
    /// the installer restarts the app itself and this process exits.
    pub restart_required: bool,
}

/// Download progress, pushed while an install is running.
#[derive(Debug, Clone, Serialize, Type, tauri_specta::Event)]
pub struct UpdateDownloadProgress {
    /// Bytes received so far.
    pub downloaded: u64,
    /// Total size, when the server sent a `Content-Length`.
    pub total: Option<u64>,
}

/// One install at a time per process: a double click must not download twice
/// or run two installers.
static INSTALLING: AtomicBool = AtomicBool::new(false);

struct InstallGuard;

impl InstallGuard {
    fn acquire() -> Result<Self, IpcError> {
        if INSTALLING.swap(true, Ordering::SeqCst) {
            return Err(IpcError::localized(
                IpcErrorCode::InvalidState,
                "update_in_progress",
                "an update is already being installed",
            ));
        }
        Ok(Self)
    }
}

impl Drop for InstallGuard {
    fn drop(&mut self) {
        INSTALLING.store(false, Ordering::SeqCst);
    }
}

/// Ask the manifest whether a newer, installable release exists. For the GUI:
/// on Windows the installer relaunches the app when it is done.
pub async fn find_update<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    channel: UpdateChannel,
) -> Result<Option<tauri_plugin_updater::Update>, IpcError> {
    find_update_with(app, channel, Relaunch::App, || {}).await
}

/// What the Windows installer starts once it has installed an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relaunch {
    /// The GUI, with the arguments it was started with (the plugin default).
    App,
    /// Nothing. For the headless CLI: the plugin would otherwise replay the
    /// CLI's own argv (`update install --yes`) through the installer's
    /// `/ARGS`, starting a second, invisible update run instead of the app.
    Nothing,
}

/// [`find_update`] for a given [`Relaunch`] choice, against `channel`'s manifest, with `before_exit` run
/// right before the process hands over to the installer and exits.
///
/// On Windows `tauri-plugin-updater` ends `Update::install` with
/// `std::process::exit(0)`, so code after [`install`] never runs there and
/// neither do destructors. `before_exit` is the last moment to report a result
/// (the CLI prints it there), and the log is flushed after it so the buffered
/// lines describing the install are not lost with the process.
pub async fn find_update_with<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    channel: UpdateChannel,
    relaunch: Relaunch,
    before_exit: impl Fn() + Send + Sync + 'static,
) -> Result<Option<tauri_plugin_updater::Update>, IpcError> {
    let url = endpoint(channel).map_err(IpcError::UpdateInstallFailed)?;
    let updater = app
        .updater_builder()
        .endpoints(vec![url])
        .and_then(|b| {
            b.restart_after_install(relaunch == Relaunch::App)
                .on_before_exit(move || {
                    tracing::info!(
                        event = "update_installer_handoff",
                        "exiting for the installer"
                    );
                    before_exit();
                    crate::logging::flush();
                })
                .build()
        })
        .map_err(|e| classify_error(&e))?;
    updater.check().await.map_err(|e| classify_error(&e))
}

/// Download, verify and install `update`, reporting progress through
/// `on_progress(downloaded, total)`.
///
/// `allow_major` is FR-087's explicit opt-in and `confirmed` the version it
/// was given for ([`check_confirmation`]); a major bump that fails either is
/// refused before a single byte is downloaded. On Windows a successful
/// install hands over to the installer and **this process exits**; elsewhere
/// the function returns and the caller offers a restart.
pub async fn install<F>(
    update: &tauri_plugin_updater::Update,
    allow_major: bool,
    confirmed: Option<&str>,
    mut on_progress: F,
) -> Result<UpdateInstallResult, IpcError>
where
    F: FnMut(u64, Option<u64>),
{
    let _guard = InstallGuard::acquire()?;

    check_confirmation(
        &update.current_version,
        &update.version,
        allow_major,
        confirmed,
    )?;

    let exe = std::env::current_exe()
        .map_err(|e| IpcError::UpdateInstallFailed(format!("cannot locate the app: {e}")))?;
    check_running_copy(&exe).map_err(|msg| {
        IpcError::localized(
            IpcErrorCode::UpdateInstallFailed,
            "not_an_installed_app",
            msg,
        )
    })?;

    tracing::info!(
        event = "update_install_started",
        from = %update.current_version,
        to = %update.version
    );
    let mut downloaded: u64 = 0;
    let bytes = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                on_progress(downloaded, total);
            },
            || {},
        )
        .await
        .map_err(|e| {
            let mapped = classify_error(&e);
            tracing::error!(event = "update_download_rejected", error = %e);
            mapped
        })?;

    // Only verified bytes reach this point: `download` checks the signature
    // and returns an error instead of the payload when it does not match.
    update.install(bytes).map_err(|e| {
        tracing::error!(event = "update_install_failed", error = %e);
        classify_error(&e)
    })?;

    tracing::info!(event = "update_installed", version = %update.version);
    Ok(UpdateInstallResult {
        installed: Some(update.version.clone()),
        restart_required: !cfg!(windows),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn fr_082_endpoint_defaults_to_the_github_latest_manifest() {
        let stable = UpdateChannel::Stable;
        assert_eq!(
            resolve_endpoint(None, stable).unwrap().as_str(),
            DEFAULT_ENDPOINT
        );
        assert_eq!(
            resolve_endpoint(Some("   "), stable).unwrap().as_str(),
            DEFAULT_ENDPOINT
        );
        assert!(DEFAULT_ENDPOINT.starts_with(env!("CARGO_PKG_REPOSITORY")));
    }

    /// FR-085: each channel reads its own manifest, both on this project's
    /// GitHub Releases. Stable is the newest non-prerelease; beta is the
    /// manifest the release workflow keeps on the fixed `channel-beta`
    /// prerelease. No nightly feed exists, so nightly reads beta.
    #[test]
    fn fr_085_endpoint_per_channel() {
        let repo = env!("CARGO_PKG_REPOSITORY");
        assert_eq!(
            default_endpoint(UpdateChannel::Stable),
            format!("{repo}/releases/latest/download/latest.json")
        );
        assert_eq!(
            default_endpoint(UpdateChannel::Beta),
            format!("{repo}/releases/download/{BETA_CHANNEL_TAG}/latest.json")
        );
        assert_eq!(
            default_endpoint(UpdateChannel::Nightly),
            default_endpoint(UpdateChannel::Beta)
        );
        assert_eq!(
            resolve_endpoint(None, UpdateChannel::Beta)
                .unwrap()
                .as_str(),
            BETA_ENDPOINT
        );
        // A test server replaces whichever channel is configured.
        for channel in [UpdateChannel::Stable, UpdateChannel::Beta] {
            assert_eq!(
                resolve_endpoint(Some("http://127.0.0.1:1/latest.json"), channel)
                    .unwrap()
                    .as_str(),
                "http://127.0.0.1:1/latest.json"
            );
        }
        // The workflow publishes the beta manifest under this very tag.
        let workflow = include_str!("../../../.github/workflows/release-channels.yml");
        assert!(workflow.contains(&format!("BETA_TAG: {BETA_CHANNEL_TAG}")));
    }

    #[test]
    fn fr_082_endpoint_can_be_overridden_for_local_testing() {
        let stable = UpdateChannel::Stable;
        let url = resolve_endpoint(Some(" http://127.0.0.1:8765/latest.json "), stable).unwrap();
        assert_eq!(url.as_str(), "http://127.0.0.1:8765/latest.json");
        assert!(resolve_endpoint(Some("not a url"), stable).is_err());
        assert!(resolve_endpoint(Some("file:///etc/passwd"), stable).is_err());
    }

    #[test]
    fn fr_087_only_a_major_jump_is_a_major_bump() {
        assert!(!is_major_bump("1.2.3", "1.3.0"));
        assert!(!is_major_bump("1.2.3", "v1.2.4"));
        assert!(!is_major_bump("0.0.1", "0.9.0"));
        assert!(is_major_bump("1.9.9", "2.0.0"));
        assert!(is_major_bump("v0.4.0", "1.0.0"));
        // When in doubt, ask.
        assert!(is_major_bump("1.0.0", "nightly"));
    }

    /// FR-087: the second confirmation belongs to the version it named. A
    /// newer major (or a manifest that disagrees with the API) installs only
    /// after its own confirmation.
    #[test]
    fn fr_087_a_major_confirmation_covers_only_the_confirmed_version() {
        // Confirmed exactly what is offered (either spelling): fine.
        assert!(check_confirmation("1.4.0", "2.0.0", true, Some("2.0.0")).is_ok());
        assert!(check_confirmation("1.4.0", "2.0.0", true, Some("v2.0.0")).is_ok());

        // The user said yes to 2.0.0; 3.0.0 is what the manifest names now.
        let err = check_confirmation("1.4.0", "3.0.0", true, Some("2.0.0")).unwrap_err();
        assert_eq!(err.reason(), Some("major_update_changed"));

        // No opt-in at all.
        let err = check_confirmation("1.4.0", "2.0.0", false, None).unwrap_err();
        assert_eq!(err.reason(), Some("major_update_unconfirmed"));

        // The CLI's `--yes` is a blanket opt-in.
        assert!(check_confirmation("1.4.0", "3.0.0", true, None).is_ok());

        // A minor update needs no confirmation, whatever was on screen.
        assert!(check_confirmation("1.4.0", "1.5.1", false, Some("1.5.0")).is_ok());
    }

    #[test]
    fn fr_083_macos_refuses_to_update_a_binary_outside_an_app_bundle() {
        let bundled = PathBuf::from("/Applications/Pluryprint.app/Contents/MacOS/pluryprint");
        assert!(check_installable_location("macos", &bundled).is_ok());
        let user = PathBuf::from("/Users/a/Applications/Pluryprint.app/Contents/MacOS/pluryprint");
        assert!(check_installable_location("macos", &user).is_ok());

        let dev = PathBuf::from("/Users/a/src/pluryprint/src-tauri/target/debug/pluryprint");
        let err = check_installable_location("macos", &dev).unwrap_err();
        assert!(err.contains("not an installed app bundle"), "{err}");

        // Windows hands over to the per-user installer wherever the exe is.
        let win = PathBuf::from(r"C:\dev\target\debug\pluryprint.exe");
        assert!(check_installable_location("windows", &win).is_ok());
    }

    /// FR-083 on Linux: only a real AppImage (named by `APPIMAGE`) or the file
    /// a package installed may update itself; anything else, which the
    /// updater would overwrite as if it were an AppImage, is refused.
    /// Unix only: an AppImage exists on Linux alone, and these are Unix paths
    /// (on Windows `/tmp/...` is not even absolute).
    #[cfg(unix)]
    #[test]
    fn fr_083_linux_updates_only_an_appimage_or_a_packaged_install() {
        let mounted = PathBuf::from("/tmp/.mount_PluryAbc/usr/bin/pluryprint");
        let file = PathBuf::from("/home/ada/Applications/Pluryprint_0.0.5_linux_x64.AppImage");
        let never = |_: &Path| -> bool { panic!("an AppImage asks no package manager") };
        assert!(check_linux_install(LinuxPackage::AppImage, &mounted, Some(&file), &never).is_ok());
        for appimage in [None, Some(Path::new("Pluryprint.AppImage"))] {
            let err = check_linux_install(LinuxPackage::AppImage, &mounted, appimage, &never)
                .unwrap_err();
            assert!(err.contains("start the .AppImage file"), "{err}");
        }

        let installed = PathBuf::from("/usr/bin/pluryprint");
        let dpkg = |p: &Path| p == Path::new("/usr/bin/pluryprint");
        assert!(check_linux_install(LinuxPackage::Deb, &installed, None, &dpkg).is_ok());
        assert!(check_linux_install(LinuxPackage::Rpm, &installed, None, &dpkg).is_ok());
        let copy = PathBuf::from("/home/ada/bin/pluryprint");
        let err = check_linux_install(LinuxPackage::Deb, &copy, None, &dpkg).unwrap_err();
        assert!(
            err.contains("not the one the .deb package installed"),
            "{err}"
        );

        // A cargo build, even with a stray APPIMAGE in the environment.
        let dev = PathBuf::from("/home/ada/src/pluryprint/src-tauri/target/debug/pluryprint");
        let err = check_linux_install(LinuxPackage::Unknown, &dev, Some(&file), &dpkg).unwrap_err();
        assert!(err.contains("neither an AppImage nor"), "{err}");
    }

    /// The running test binary is no installed app on any OS: a cargo build
    /// must never update itself.
    #[test]
    fn fr_083_a_cargo_build_refuses_to_update_itself() {
        let exe = std::env::current_exe().unwrap();
        if cfg!(windows) {
            // Windows hands over to the per-user installer from anywhere.
            assert!(check_running_copy(&exe).is_ok());
        } else {
            assert!(check_running_copy(&exe).is_err(), "{}", exe.display());
        }
    }

    /// Every Linux install failure the updater can report reads as what it
    /// is, with its own reason, instead of the plugin's bare error text.
    #[test]
    fn fr_082_linux_install_failures_are_readable() {
        use tauri_plugin_updater::Error as E;
        for (err, reason) in [
            (E::InvalidUpdaterFormat, "wrong_package_format"),
            (E::BinaryNotFoundInArchive, "wrong_package_format"),
            (E::TempDirNotOnSameMountPoint, "appimage_not_writable"),
            (E::AuthenticationFailed, "auth_cancelled"),
            (E::DebInstallFailed, "package_install_failed"),
            (E::PackageInstallFailed, "package_install_failed"),
            (E::TempDirNotFound, "install_failed"),
            (
                E::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
                "install_failed",
            ),
            (
                E::TargetsNotFound(vec!["linux-x86_64-deb".into()]),
                "no_platform_update",
            ),
        ] {
            let got = classify_error(&err);
            assert_eq!(got.code(), IpcErrorCode::UpdateInstallFailed, "{err:?}");
            assert_eq!(got.reason(), Some(reason), "{err:?}");
        }
        let kept = classify_error(&E::PreviousAppNotRestored(PathBuf::from("/tmp/x/app")));
        assert_eq!(kept.reason(), Some("previous_app_not_restored"));
        assert!(kept.message().contains("/tmp/x/app"), "{kept:?}");
        let targets = classify_error(&E::TargetsNotFound(vec![
            "linux-aarch64-deb".into(),
            "linux-aarch64".into(),
        ]));
        assert!(
            targets.message().contains("linux-aarch64-deb"),
            "{targets:?}"
        );
    }

    #[test]
    fn fr_082_a_bad_signature_is_reported_as_such() {
        use tauri_plugin_updater::Error as E;
        for err in [
            E::SignatureUtf8("zzz".into()),
            E::MissingSignedVersion,
            E::SignedVersionMismatch {
                signed: "1.0.0".into(),
                announced: "2.0.0".into(),
            },
        ] {
            let got = classify_error(&err);
            assert_eq!(got.code(), IpcErrorCode::UpdateSignatureInvalid, "{err:?}");
            assert_eq!(got.reason(), Some("signature_rejected"));
            assert!(got.message().contains("NOT installed"), "{got:?}");
        }
        let network = classify_error(&E::Network("503".into()));
        assert_eq!(network.code(), IpcErrorCode::UpdateInstallFailed);
        assert_eq!(network.reason(), Some("download_failed"));
        let missing = classify_error(&E::ReleaseNotFound);
        assert_eq!(missing.code(), IpcErrorCode::UpdateInstallFailed);
        assert_eq!(missing.reason(), Some("no_platform_update"));
    }

    /// The shipped configuration is what makes FR-082/083 true at all: a public
    /// key to verify against, signed updater artifacts, the documented
    /// endpoint, and a per-user (no admin) Windows installer.
    #[test]
    fn fr_083_shipped_config_signs_updates_and_installs_per_user() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let updater = &conf["plugins"]["updater"];

        let pubkey = updater["pubkey"].as_str().expect("an updater public key");
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(pubkey)
            .expect("the public key is base64");
        let text = String::from_utf8(decoded).unwrap();
        assert!(text.contains("minisign public key"), "{text}");

        assert_eq!(updater["endpoints"][0], DEFAULT_ENDPOINT);
        assert_eq!(conf["bundle"]["createUpdaterArtifacts"], true);
        assert_eq!(
            conf["bundle"]["windows"]["nsis"]["installMode"],
            "currentUser"
        );
        assert!(
            updater.get("dangerousInsecureTransportProtocol").is_none(),
            "release builds must keep refusing plain-http endpoints"
        );
        // The manifest is not signed; only the version inside the artifact's
        // signature ties `version` to the file it points at. Without this a
        // tampered latest.json could pair a new number with an older release.
        assert_eq!(updater["requireSignedVersion"], true);
        assert!(
            updater.get("allowDowngrades").is_none(),
            "an update must be newer than the running version"
        );
        // ...and the plugin reads it under that name: a misspelt key would be
        // silently ignored and leave the check off.
        let parsed: tauri_plugin_updater::Config =
            serde_json::from_value(updater.clone()).expect("the plugin accepts its config");
        assert!(parsed.require_signed_version);
        assert!(!parsed.allow_downgrades);
    }

    /// ADR-0008: the per-user NSIS setup is the only Windows installer. With
    /// `"targets": "all"` the bundler also built a per-machine WiX MSI, which
    /// installs a second copy under Program Files that NSIS updates never
    /// reach.
    #[test]
    fn fr_083_no_per_machine_msi_is_bundled() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let targets = conf["bundle"]["targets"]
            .as_array()
            .expect("an explicit target list, never \"all\"");
        let names: Vec<&str> = targets.iter().filter_map(|t| t.as_str()).collect();
        assert!(names.contains(&"nsis"), "{names:?}");
        assert!(!names.contains(&"msi"), "{names:?}");
        assert!(
            names.contains(&"app") && names.contains(&"dmg"),
            "{names:?}"
        );
    }

    #[test]
    fn fr_082_only_one_install_runs_at_a_time() {
        let first = InstallGuard::acquire().unwrap();
        let busy = InstallGuard::acquire()
            .err()
            .expect("second install refused");
        assert_eq!(busy.code(), IpcErrorCode::InvalidState);
        assert_eq!(busy.reason(), Some("update_in_progress"));
        drop(first);
        assert!(InstallGuard::acquire().is_ok());
    }
}
