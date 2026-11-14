//! Registering the bridge with the browsers (FR-127).
//!
//! A browser finds a Native Messaging host through a JSON manifest named
//! after the host (`com.maecly.pluryprint.json`) that gives the executable's
//! path and the extension origins allowed to start it:
//!
//! | OS      | Where the browser looks (per user)                                         |
//! | ------- | -------------------------------------------------------------------------- |
//! | macOS   | `~/Library/Application Support/<browser>/NativeMessagingHosts/`            |
//! | Linux   | `~/.config/<browser>/NativeMessagingHosts/` (`$XDG_CONFIG_HOME`)           |
//! | Windows | the file a `HKCU\Software\<browser>\NativeMessagingHosts\<host>` key names |
//!
//! On Windows the installer writes the manifest next to the bridge and the
//! three registry keys (Chrome, Edge, Chromium; Brave reads Chrome's), and the
//! uninstaller removes them (`windows/installer-hooks.nsh`). On macOS there is
//! no installer, so the app registers itself at every start, for each browser
//! whose support folder exists, rewriting a manifest whose path no longer
//! points at the running app ([`register_at_startup`]). On Linux
//! `pluryprint-cli bridge install` writes the user-level manifests (the .deb
//! is not touched; Linux is not a promised platform for the integration).
//!
//! Everything below that is not an OS call is a pure function of the
//! platform, the home folder and the bridge path, so it is tested on every OS.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{HOST_NAME, PLURYFACT_ORIGIN};

/// The manifest's file name.
pub fn manifest_file_name() -> String {
    format!("{HOST_NAME}.json")
}

/// The browsers the bridge registers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum Browser {
    /// Google Chrome.
    Chrome,
    /// Microsoft Edge.
    Edge,
    /// Brave.
    Brave,
    /// Chromium.
    Chromium,
}

impl Browser {
    /// Every browser, in display order.
    pub const ALL: [Browser; 4] = [
        Browser::Chrome,
        Browser::Edge,
        Browser::Brave,
        Browser::Chromium,
    ];

    /// Lowercase name, as the CLI spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Browser::Chrome => "chrome",
            Browser::Edge => "edge",
            Browser::Brave => "brave",
            Browser::Chromium => "chromium",
        }
    }

    /// Parse the CLI's spelling.
    pub fn parse(s: &str) -> Option<Browser> {
        Browser::ALL
            .into_iter()
            .find(|b| b.as_str().eq_ignore_ascii_case(s))
    }
}

/// The OS layout to compute locations for (injected, so every layout is
/// tested on every OS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// macOS.
    MacOs,
    /// Linux and other XDG desktops.
    Linux,
    /// Windows.
    Windows,
}

impl Platform {
    /// The OS this binary runs on.
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }
}

/// The browser's per-user data folder: what "is this browser installed for
/// this user" is judged by on macOS and Linux. `config_home` is
/// `$XDG_CONFIG_HOME` (or `~/.config`) and is only used on Linux. `None` on
/// Windows, where the registry is the location.
pub fn browser_support_dir(
    platform: Platform,
    home: &Path,
    config_home: &Path,
    browser: Browser,
) -> Option<PathBuf> {
    match platform {
        Platform::MacOs => {
            let base = home.join("Library/Application Support");
            Some(match browser {
                Browser::Chrome => base.join("Google/Chrome"),
                Browser::Edge => base.join("Microsoft Edge"),
                Browser::Brave => base.join("BraveSoftware/Brave-Browser"),
                Browser::Chromium => base.join("Chromium"),
            })
        }
        Platform::Linux => Some(match browser {
            Browser::Chrome => config_home.join("google-chrome"),
            Browser::Edge => config_home.join("microsoft-edge"),
            Browser::Brave => config_home.join("BraveSoftware/Brave-Browser"),
            Browser::Chromium => config_home.join("chromium"),
        }),
        Platform::Windows => None,
    }
}

/// Where the manifest for `browser` goes on macOS and Linux. On Windows every
/// browser shares the manifest next to the bridge ([`windows_manifest_path`]).
pub fn manifest_path(
    platform: Platform,
    home: &Path,
    config_home: &Path,
    bridge: &Path,
    browser: Browser,
) -> PathBuf {
    match browser_support_dir(platform, home, config_home, browser) {
        Some(dir) => dir.join("NativeMessagingHosts").join(manifest_file_name()),
        None => windows_manifest_path(bridge),
    }
}

/// The Windows manifest: next to the bridge, in the install folder, where
/// the installer writes it too.
pub fn windows_manifest_path(bridge: &Path) -> PathBuf {
    bridge.with_file_name(manifest_file_name())
}

/// The `HKCU` subkey whose default value names the manifest for `browser` on
/// Windows. Brave reads Chrome's.
pub fn windows_registry_key(browser: Browser) -> String {
    let vendor = match browser {
        Browser::Chrome | Browser::Brave => "Google\\Chrome",
        Browser::Edge => "Microsoft\\Edge",
        Browser::Chromium => "Chromium",
    };
    format!("Software\\{vendor}\\NativeMessagingHosts\\{HOST_NAME}")
}

/// The manifest file's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// [`HOST_NAME`].
    pub name: String,
    /// Shown nowhere; for whoever opens the file.
    pub description: String,
    /// Absolute path of the bridge (on Windows, relative to the manifest).
    pub path: String,
    /// Always `stdio`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The extensions allowed to start the bridge.
    pub allowed_origins: Vec<String>,
}

impl Manifest {
    /// A manifest for `path`, allowing `origins`.
    pub fn new(path: String, origins: Vec<String>) -> Self {
        Self {
            name: HOST_NAME.to_string(),
            description: "Pluryprint bridge for PluryFact (prints the documents PluryFact sends)"
                .to_string(),
            path,
            kind: "stdio".to_string(),
            allowed_origins: origins,
        }
    }

    /// The bytes written to disk (pretty JSON, trailing newline).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = serde_json::to_vec_pretty(self).expect("a manifest serializes");
        out.push(b'\n');
        out
    }
}

/// Whether `origin` is a Chrome extension origin: `chrome-extension://`, a
/// 32-letter id from `a` to `p`, and a trailing slash. Wildcards and anything
/// else are refused (the browser accepts no wildcard either).
pub fn is_extension_origin(origin: &str) -> bool {
    origin
        .strip_prefix("chrome-extension://")
        .and_then(|rest| rest.strip_suffix('/'))
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b)))
}

/// The origins a manifest allows: PluryFact's first, then `extra` (already
/// validated), without duplicates.
pub fn origins_with(extra: &[String]) -> Vec<String> {
    let mut out = vec![PLURYFACT_ORIGIN.to_string()];
    for o in extra {
        if !out.contains(o) {
            out.push(o.clone());
        }
    }
    out
}

/// Read a manifest file, if one is there and parses.
pub fn read_manifest(path: &Path) -> Option<Manifest> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > 64 * 1024 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// The registration state of one browser, for `bridge status` and the
/// Settings screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct BrowserStatus {
    /// Which browser.
    pub browser: Browser,
    /// Whether the browser seems installed for this user (its support folder
    /// exists on macOS/Linux; on Windows this is not checked and is `true`).
    pub detected: bool,
    /// Where its manifest is (or would be) written.
    pub manifest_path: String,
    /// Whether that manifest exists and names this host.
    pub registered: bool,
    /// Whether the manifest's `path` is the bridge this process would
    /// register (`false` after the app moved, until it registers again).
    pub points_here: bool,
    /// Whether the executable the manifest names exists.
    pub binary_exists: bool,
    /// The origins the manifest allows.
    pub allowed_origins: Vec<String>,
}

/// Inputs every registration operation needs.
#[derive(Debug, Clone)]
pub struct Target {
    /// Which OS layout.
    pub platform: Platform,
    /// The user's home folder.
    pub home: PathBuf,
    /// `$XDG_CONFIG_HOME` or `~/.config` (Linux only).
    pub config_home: PathBuf,
    /// The bridge executable to register.
    pub bridge: PathBuf,
}

impl Target {
    /// This machine and this install: the bridge next to the running
    /// executable.
    pub fn current() -> Option<Target> {
        let dirs = directories::BaseDirs::new()?;
        let exe = std::env::current_exe().ok()?;
        Some(Target {
            platform: Platform::current(),
            home: dirs.home_dir().to_path_buf(),
            config_home: dirs.config_dir().to_path_buf(),
            bridge: exe.with_file_name(super::launch::BRIDGE_EXE),
        })
    }

    fn manifest_path(&self, browser: Browser) -> PathBuf {
        manifest_path(
            self.platform,
            &self.home,
            &self.config_home,
            &self.bridge,
            browser,
        )
    }

    /// The `path` the manifest records: absolute, except on Windows where
    /// the manifest sits next to the bridge and the bare file name is what
    /// the installer writes too.
    fn manifest_exe_field(&self) -> String {
        match self.platform {
            Platform::Windows => super::launch::BRIDGE_EXE.to_string(),
            _ => self.bridge.to_string_lossy().into_owned(),
        }
    }

    /// Whether the browser seems installed for this user.
    pub fn detected(&self, browser: Browser) -> bool {
        browser_support_dir(self.platform, &self.home, &self.config_home, browser)
            .map(|d| d.is_dir())
            .unwrap_or(true)
    }

    /// The status of one browser.
    pub fn status(&self, browser: Browser) -> BrowserStatus {
        let path = self.manifest_path(browser);
        let manifest = read_manifest(&path).filter(|m| m.name == HOST_NAME);
        #[cfg(windows)]
        let registered_key = self.platform != Platform::Windows
            || windows_reg::read_default(&windows_registry_key(browser))
                .is_some_and(|v| Path::new(&v) == path);
        #[cfg(not(windows))]
        let registered_key = true;
        let exe = manifest.as_ref().map(|m| {
            let p = PathBuf::from(&m.path);
            if p.is_absolute() {
                p
            } else {
                path.with_file_name(p)
            }
        });
        BrowserStatus {
            browser,
            detected: self.detected(browser),
            manifest_path: path.to_string_lossy().into_owned(),
            registered: manifest.is_some() && registered_key,
            points_here: manifest
                .as_ref()
                .is_some_and(|m| m.path == self.manifest_exe_field()),
            binary_exists: exe.as_deref().is_some_and(Path::is_file),
            allowed_origins: manifest.map(|m| m.allowed_origins).unwrap_or_default(),
        }
    }

    /// Write (or rewrite) the manifest for `browser`, allowing PluryFact plus
    /// `extra` plus any extra origin the existing manifest already allowed
    /// (so re-registering never drops a development origin someone added).
    /// Returns whether anything was written: an identical manifest is left
    /// alone (registration is idempotent).
    ///
    /// This holds for the CLI and Reconnect only. On Windows the setup
    /// (`windows/installer-hooks.nsh`) rewrites the shared manifest with
    /// PluryFact's origin alone on every install and update, so an extra
    /// origin has to be added again afterwards (ipc-and-cli.md §6).
    pub fn install(&self, browser: Browser, extra: &[String]) -> std::io::Result<bool> {
        let path = self.manifest_path(browser);
        let existing = read_manifest(&path).filter(|m| m.name == HOST_NAME);
        let mut keep: Vec<String> = existing
            .as_ref()
            .map(|m| {
                m.allowed_origins
                    .iter()
                    .filter(|o| is_extension_origin(o))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        keep.extend(extra.iter().cloned());
        let manifest = Manifest::new(self.manifest_exe_field(), origins_with(&keep));
        let wrote = if existing.as_ref() == Some(&manifest) {
            false
        } else {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            write_atomically(&path, &manifest.to_bytes())?;
            true
        };
        #[cfg(windows)]
        if self.platform == Platform::Windows {
            windows_reg::write_default(&windows_registry_key(browser), &path.to_string_lossy())?;
        }
        Ok(wrote)
    }

    /// Remove the registration for `browser`. On Windows the shared manifest
    /// is removed only once no browser key names it any more.
    pub fn uninstall(&self, browser: Browser) -> std::io::Result<()> {
        let path = self.manifest_path(browser);
        #[cfg(windows)]
        if self.platform == Platform::Windows {
            windows_reg::delete_key(&windows_registry_key(browser))?;
            let still_used = Browser::ALL
                .iter()
                .any(|b| windows_reg::read_default(&windows_registry_key(*b)).is_some());
            if still_used {
                return Ok(());
            }
        }
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// Write `bytes` to `path` through a temporary file in the same folder and a
/// rename, so a browser never reads half a manifest.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::io::Write::write_all(&mut tmp, bytes)?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Whether `bridge` runs from a translocated copy (macOS App Translocation:
/// an app opened from the disk image or the Downloads folder without being
/// moved runs from a random read-only path). Registering that path would
/// break as soon as the volume goes away, so the app does not.
pub fn is_translocated(bridge: &Path) -> bool {
    bridge.to_string_lossy().contains("/AppTranslocation/")
}

/// Whether `bridge` sits inside a macOS app bundle (`….app/Contents/MacOS/`).
pub fn is_app_bundle(bridge: &Path) -> bool {
    bridge
        .parent()
        .is_some_and(|d| d.ends_with("Contents/MacOS"))
        && bridge
            .ancestors()
            .nth(3)
            .and_then(|a| a.extension())
            .is_some_and(|e| e == "app")
}

/// Whether the app should register at startup on this platform: macOS only
/// (Windows has the installer; Linux has no promised registration).
pub fn registers_at_startup(platform: Platform) -> bool {
    platform == Platform::MacOs
}

/// What the startup registration did, for the log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StartupOutcome {
    /// Browsers whose manifest was written.
    pub written: Vec<Browser>,
    /// Browsers already registered for this bridge.
    pub unchanged: Vec<Browser>,
    /// Browsers not installed for this user.
    pub skipped: Vec<Browser>,
    /// Why nothing was attempted, if so.
    pub not_attempted: Option<&'static str>,
}

/// The macOS startup registration: for every browser whose support folder
/// exists, make sure its manifest points at this app's bridge. Nothing is
/// done for a translocated app or when the bridge is not next to the app.
pub fn register_at_startup(target: &Target) -> StartupOutcome {
    let mut out = StartupOutcome::default();
    if !registers_at_startup(target.platform) {
        out.not_attempted = Some("platform");
        return out;
    }
    if is_translocated(&target.bridge) {
        out.not_attempted = Some("translocated");
        return out;
    }
    // Only an installed app bundle registers itself: a development build
    // (`tauri dev`, `target/debug`) must not point the developer's browsers
    // at a build folder. `bridge install` still does it on request.
    if !is_app_bundle(&target.bridge) {
        out.not_attempted = Some("not_bundled");
        return out;
    }
    if !target.bridge.is_file() {
        out.not_attempted = Some("no_bridge");
        return out;
    }
    for browser in Browser::ALL {
        if !target.detected(browser) {
            out.skipped.push(browser);
            continue;
        }
        match target.install(browser, &[]) {
            Ok(true) => out.written.push(browser),
            Ok(false) => out.unchanged.push(browser),
            Err(e) => tracing::warn!(
                event = "bridge_register_failed",
                browser = browser.as_str(),
                error = %e
            ),
        }
    }
    out
}

/// The origins the running bridge accepts: PluryFact's, plus every origin
/// listed by a manifest of this host found where the browsers look (the
/// extra development origins `bridge install --extra-origin` added), plus,
/// in a test run only, a test manifest in `test_dir` (the folder of
/// [`super::DATA_DIR_ENV`]). Production passes `None`: the app's data
/// directory is never read for origins, so `bridge install --extra-origin`
/// stays the only way to add one, and `bridge status`/`uninstall` see them
/// all.
pub fn allowed_origins(test_dir: Option<&Path>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    if let Some(target) = Target::current() {
        for browser in Browser::ALL {
            if let Some(m) = read_manifest(&target.manifest_path(browser)) {
                found.extend(m.allowed_origins);
            }
        }
    }
    if let Some(dir) = test_dir {
        found.extend(installed_origins_in(dir));
    }
    let found: Vec<String> = found
        .into_iter()
        .filter(|o| is_extension_origin(o))
        .collect();
    origins_with(&found)
}

/// Origins from a manifest of this host stored directly in `dir` (the
/// test-only data directory of [`super::DATA_DIR_ENV`]).
pub fn installed_origins_in(dir: &Path) -> Vec<String> {
    read_manifest(&dir.join(manifest_file_name()))
        .filter(|m| m.name == HOST_NAME)
        .map(|m| m.allowed_origins)
        .unwrap_or_default()
}

#[cfg(windows)]
mod windows_reg {
    //! The three HKCU operations the Windows registration needs.
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The default value of `HKCU\<subkey>`, if it is a string.
    pub fn read_default(subkey: &str) -> Option<String> {
        let key = wide(subkey);
        let mut size = 0u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                PCWSTR::null(),
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            )
        };
        if rc != ERROR_SUCCESS || size < 2 {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                PCWSTR::null(),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    /// Create `HKCU\<subkey>` if needed and set its default value.
    pub fn write_default(subkey: &str, value: &str) -> std::io::Result<()> {
        let key = wide(subkey);
        let mut handle = HKEY::default();
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut handle,
                None,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(rc.0 as i32));
        }
        let data: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
        let rc = unsafe { RegSetValueExW(handle, PCWSTR::null(), None, REG_SZ, Some(&data)) };
        unsafe {
            let _ = RegCloseKey(handle);
        }
        if rc != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(rc.0 as i32));
        }
        Ok(())
    }

    /// Delete `HKCU\<subkey>` and everything under it; a missing key is fine.
    pub fn delete_key(subkey: &str) -> std::io::Result<()> {
        let key = wide(subkey);
        let rc = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr())) };
        if rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(rc.0 as i32))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(platform: Platform, root: &Path) -> Target {
        let bridge = root
            .join("Pluryprint.app/Contents/MacOS")
            .join(super::super::launch::BRIDGE_EXE);
        Target {
            platform,
            home: root.join("home"),
            config_home: root.join("home/.config"),
            bridge,
        }
    }

    #[test]
    fn fr_127_manifest_locations_per_os() {
        let home = Path::new("/Users/ana");
        let xdg = Path::new("/home/ana/.config");
        let bridge = Path::new("/Applications/Pluryprint.app/Contents/MacOS/pluryprint-bridge");
        let mac = |b| manifest_path(Platform::MacOs, home, xdg, bridge, b);
        assert_eq!(
            mac(Browser::Chrome),
            Path::new("/Users/ana/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.maecly.pluryprint.json")
        );
        assert_eq!(
            mac(Browser::Edge),
            Path::new("/Users/ana/Library/Application Support/Microsoft Edge/NativeMessagingHosts/com.maecly.pluryprint.json")
        );
        assert_eq!(
            mac(Browser::Brave),
            Path::new("/Users/ana/Library/Application Support/BraveSoftware/Brave-Browser/NativeMessagingHosts/com.maecly.pluryprint.json")
        );
        assert_eq!(
            mac(Browser::Chromium),
            Path::new("/Users/ana/Library/Application Support/Chromium/NativeMessagingHosts/com.maecly.pluryprint.json")
        );
        let linux = |b| manifest_path(Platform::Linux, home, xdg, bridge, b);
        assert_eq!(
            linux(Browser::Chrome),
            Path::new(
                "/home/ana/.config/google-chrome/NativeMessagingHosts/com.maecly.pluryprint.json"
            )
        );
        assert_eq!(
            linux(Browser::Edge),
            Path::new(
                "/home/ana/.config/microsoft-edge/NativeMessagingHosts/com.maecly.pluryprint.json"
            )
        );
        assert_eq!(
            linux(Browser::Brave),
            Path::new("/home/ana/.config/BraveSoftware/Brave-Browser/NativeMessagingHosts/com.maecly.pluryprint.json")
        );
        assert_eq!(
            linux(Browser::Chromium),
            Path::new("/home/ana/.config/chromium/NativeMessagingHosts/com.maecly.pluryprint.json")
        );
        // Windows: one manifest next to the bridge for every browser.
        for b in Browser::ALL {
            assert_eq!(
                manifest_path(Platform::Windows, home, xdg, bridge, b),
                bridge.with_file_name("com.maecly.pluryprint.json")
            );
        }
    }

    #[test]
    fn fr_127_windows_registry_keys() {
        assert_eq!(
            windows_registry_key(Browser::Chrome),
            r"Software\Google\Chrome\NativeMessagingHosts\com.maecly.pluryprint"
        );
        assert_eq!(
            windows_registry_key(Browser::Brave),
            windows_registry_key(Browser::Chrome),
            "Brave reads Chrome's key on Windows"
        );
        assert_eq!(
            windows_registry_key(Browser::Edge),
            r"Software\Microsoft\Edge\NativeMessagingHosts\com.maecly.pluryprint"
        );
        assert_eq!(
            windows_registry_key(Browser::Chromium),
            r"Software\Chromium\NativeMessagingHosts\com.maecly.pluryprint"
        );
    }

    #[test]
    fn fr_127_the_manifest_has_the_shape_chrome_requires() {
        let m = Manifest::new("/x/pluryprint-bridge".into(), origins_with(&[]));
        let v: serde_json::Value = serde_json::from_slice(&m.to_bytes()).unwrap();
        assert_eq!(v["name"], "com.maecly.pluryprint");
        assert_eq!(v["type"], "stdio");
        assert_eq!(v["path"], "/x/pluryprint-bridge");
        assert_eq!(
            v["allowed_origins"],
            serde_json::json!(["chrome-extension://ibgodfpcihlddbbfaoooehfgbmahipbj/"])
        );
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys.len(), 5, "{keys:?}");
        // Chrome's rule for host names: lowercase, digits, `_` and `.`.
        assert!(HOST_NAME
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.'));
    }

    #[test]
    fn fr_127_only_exact_extension_origins_are_accepted() {
        assert!(is_extension_origin(PLURYFACT_ORIGIN));
        assert!(is_extension_origin(&format!(
            "chrome-extension://{}/",
            "a".repeat(32)
        )));
        for bad in [
            "chrome-extension://*/",
            "chrome-extension://ibgodfpcihlddbbfaoooehfgbmahipbj",
            "chrome-extension://ibgodfpcihlddbbfaoooehfgbmahipbz/",
            "chrome-extension://IBGODFPCIHLDDBBFAOOOEHFGBMAHIPBJ/",
            "https://pluryfact.maecly.com/",
            "moz-extension://ibgodfpcihlddbbfaoooehfgbmahipbj/",
            "",
        ] {
            assert!(!is_extension_origin(bad), "{bad}");
        }
        let extra = format!("chrome-extension://{}/", "b".repeat(32));
        assert_eq!(
            origins_with(&[extra.clone(), PLURYFACT_ORIGIN.into(), extra.clone()]),
            [PLURYFACT_ORIGIN.to_string(), extra]
        );
    }

    /// Install, status and uninstall against a temporary home, with the
    /// macOS layout (pure file operations, so it runs on every OS).
    #[test]
    fn fr_127_install_is_idempotent_and_keeps_extra_origins() {
        let tmp = tempfile::tempdir().unwrap();
        let t = target(Platform::MacOs, tmp.path());
        std::fs::create_dir_all(t.bridge.parent().unwrap()).unwrap();
        std::fs::write(&t.bridge, b"#!").unwrap();

        let before = t.status(Browser::Chrome);
        assert!(!before.registered && !before.detected);

        let extra = format!("chrome-extension://{}/", "c".repeat(32));
        assert!(t
            .install(Browser::Chrome, std::slice::from_ref(&extra))
            .unwrap());
        let s = t.status(Browser::Chrome);
        assert!(s.registered && s.points_here && s.binary_exists, "{s:?}");
        assert_eq!(
            s.allowed_origins,
            [PLURYFACT_ORIGIN.to_string(), extra.clone()]
        );

        // Again, without extras: nothing to write, and the extra one stays.
        assert!(!t.install(Browser::Chrome, &[]).unwrap());
        assert_eq!(t.status(Browser::Chrome).allowed_origins.len(), 2);

        // The app moved: the manifest is rewritten to the new path.
        let moved = Target {
            bridge: tmp.path().join("elsewhere/pluryprint-bridge"),
            ..t.clone()
        };
        assert!(!moved.status(Browser::Chrome).points_here);
        assert!(moved.install(Browser::Chrome, &[]).unwrap());
        assert!(moved.status(Browser::Chrome).points_here);
        assert!(
            !moved.status(Browser::Chrome).binary_exists,
            "nothing there yet"
        );

        t.uninstall(Browser::Chrome).unwrap();
        assert!(!t.status(Browser::Chrome).registered);
        t.uninstall(Browser::Chrome).unwrap();
    }

    #[test]
    fn fr_127_startup_registration_covers_detected_browsers_only() {
        let tmp = tempfile::tempdir().unwrap();
        let t = target(Platform::MacOs, tmp.path());
        std::fs::create_dir_all(t.bridge.parent().unwrap()).unwrap();
        std::fs::write(&t.bridge, b"#!").unwrap();
        std::fs::create_dir_all(t.home.join("Library/Application Support/Google/Chrome")).unwrap();
        std::fs::create_dir_all(
            t.home
                .join("Library/Application Support/BraveSoftware/Brave-Browser"),
        )
        .unwrap();

        let first = register_at_startup(&t);
        assert_eq!(first.written, [Browser::Chrome, Browser::Brave]);
        assert_eq!(first.skipped, [Browser::Edge, Browser::Chromium]);
        let second = register_at_startup(&t);
        assert!(second.written.is_empty());
        assert_eq!(second.unchanged, [Browser::Chrome, Browser::Brave]);

        // Never from a translocated copy, never on another OS.
        let translocated = Target {
            bridge: PathBuf::from("/private/var/folders/x/AppTranslocation/ABC/d/Pluryprint.app/Contents/MacOS/pluryprint-bridge"),
            ..t.clone()
        };
        assert_eq!(
            register_at_startup(&translocated).not_attempted,
            Some("translocated")
        );
        let linux = Target {
            platform: Platform::Linux,
            ..t.clone()
        };
        assert_eq!(register_at_startup(&linux).not_attempted, Some("platform"));
        let dev = Target {
            bridge: tmp.path().join("target/debug/pluryprint-bridge"),
            ..t.clone()
        };
        assert_eq!(register_at_startup(&dev).not_attempted, Some("not_bundled"));
        let missing = Target {
            bridge: tmp.path().join("Gone.app/Contents/MacOS/pluryprint-bridge"),
            ..t
        };
        assert_eq!(
            register_at_startup(&missing).not_attempted,
            Some("no_bridge")
        );
    }

    #[test]
    fn fr_127_allowed_origins_always_start_with_pluryfact() {
        let tmp = tempfile::tempdir().unwrap();
        let extra = format!("chrome-extension://{}/", "d".repeat(32));
        let m = Manifest::new(
            "/x".into(),
            vec![extra.clone(), "chrome-extension://*/".into()],
        );
        std::fs::write(tmp.path().join(manifest_file_name()), m.to_bytes()).unwrap();
        let allowed = allowed_origins(Some(tmp.path()));
        assert_eq!(allowed[0], PLURYFACT_ORIGIN);
        assert!(allowed.contains(&extra));
        assert!(!allowed.iter().any(|o| o.contains('*')), "{allowed:?}");
        // Production reads no manifest from the data dir: a file planted
        // there cannot add an origin.
        assert!(!allowed_origins(None).contains(&extra));
    }

    #[test]
    fn fr_127_only_an_app_bundle_counts_as_installed() {
        assert!(is_app_bundle(Path::new(
            "/Applications/Pluryprint.app/Contents/MacOS/pluryprint-bridge"
        )));
        assert!(!is_app_bundle(Path::new(
            "/repo/src-tauri/target/debug/pluryprint-bridge"
        )));
        assert!(!is_app_bundle(Path::new(
            "/x/Contents/MacOS/pluryprint-bridge"
        )));
    }

    #[test]
    fn fr_127_browser_names_round_trip() {
        for b in Browser::ALL {
            assert_eq!(Browser::parse(b.as_str()), Some(b));
            assert_eq!(Browser::parse(&b.as_str().to_uppercase()), Some(b));
        }
        assert_eq!(Browser::parse("firefox"), None);
    }
}
