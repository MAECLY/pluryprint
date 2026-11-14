//! The Settings screen's "Browser integration" section (FR-127): whether the
//! bridge Pluryfact prints through is registered with each browser, and a
//! "Reconnect" that registers it again.
//!
//! Both commands are thin wrappers over [`crate::bridge::register`]; nothing
//! here talks to a browser or to the network.

use serde::{Deserialize, Serialize};

use crate::bridge::register::{self, Browser, BrowserStatus, Platform, Target};
use crate::util::error::IpcError;

/// What the Settings section shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct BridgeOverview {
    /// `windows`, `macos` or `linux`.
    pub platform: String,
    /// Whether the bridge executable is installed next to the app.
    pub installed: bool,
    /// Whether the app runs from a translocated copy (macOS): registration is
    /// then refused until the app is moved to Applications.
    pub translocated: bool,
    /// One row per browser, in display order.
    pub browsers: Vec<BrowserStatus>,
}

fn platform_name(p: Platform) -> &'static str {
    match p {
        Platform::Windows => "windows",
        Platform::MacOs => "macos",
        Platform::Linux => "linux",
    }
}

/// The overview for `target`.
pub fn overview_at(target: &Target) -> BridgeOverview {
    BridgeOverview {
        platform: platform_name(target.platform).to_string(),
        installed: target.bridge.is_file(),
        translocated: register::is_translocated(&target.bridge),
        browsers: Browser::ALL.iter().map(|b| target.status(*b)).collect(),
    }
}

/// Register again with every browser found for this user (on Windows, with
/// all of them: the registry is the only place to look), keeping any extra
/// origin a manifest already allows, then report the overview.
pub fn reconnect_at(target: &Target) -> Result<BridgeOverview, IpcError> {
    if register::is_translocated(&target.bridge) {
        return Err(IpcError::localized(
            crate::util::error::IpcErrorCode::InvalidRequest,
            "bridge_translocated",
            "Pluryprint is running from a temporary location; move it to Applications first",
        ));
    }
    if !target.bridge.is_file() {
        return Err(IpcError::localized(
            crate::util::error::IpcErrorCode::Internal,
            "bridge_missing",
            "the browser bridge is not installed next to the app",
        ));
    }
    for browser in Browser::ALL {
        if !target.detected(browser) {
            continue;
        }
        target.install(browser, &[]).map_err(|e| {
            IpcError::Internal(format!("cannot register with {}: {e}", browser.as_str()))
        })?;
    }
    Ok(overview_at(target))
}

fn current() -> Result<Target, IpcError> {
    Target::current()
        .ok_or_else(|| IpcError::Internal("cannot resolve the home folder or the app path".into()))
}

/// The registration state of the browser bridge.
#[tauri::command]
#[specta::specta]
pub async fn bridge_status() -> Result<BridgeOverview, IpcError> {
    super::run_blocking(|| Ok(overview_at(&current()?))).await
}

/// Register the browser bridge again (the "Reconnect" button).
#[tauri::command]
#[specta::specta]
pub async fn bridge_reconnect() -> Result<BridgeOverview, IpcError> {
    super::run_blocking(|| {
        let target = current()?;
        let out = reconnect_at(&target);
        tracing::info!(event = "bridge_reconnect", ok = out.is_ok());
        out
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr_127_reconnect_registers_the_browsers_found() {
        let tmp = tempfile::tempdir().unwrap();
        let bridge = tmp
            .path()
            .join("MacOS")
            .join(crate::bridge::launch::BRIDGE_EXE);
        std::fs::create_dir_all(bridge.parent().unwrap()).unwrap();
        let target = Target {
            platform: Platform::MacOs,
            home: tmp.path().join("home"),
            config_home: tmp.path().join("home/.config"),
            bridge: bridge.clone(),
        };
        // No bridge yet: refused, nothing written.
        let err = reconnect_at(&target).unwrap_err();
        assert_eq!(err.reason(), Some("bridge_missing"));

        std::fs::write(&bridge, b"#!").unwrap();
        std::fs::create_dir_all(
            target
                .home
                .join("Library/Application Support/Microsoft Edge"),
        )
        .unwrap();
        let o = reconnect_at(&target).unwrap();
        assert_eq!(o.platform, "macos");
        assert!(o.installed && !o.translocated);
        let edge = o
            .browsers
            .iter()
            .find(|b| b.browser == Browser::Edge)
            .unwrap();
        assert!(edge.detected && edge.registered && edge.points_here);
        assert!(o
            .browsers
            .iter()
            .filter(|b| b.browser != Browser::Edge)
            .all(|b| !b.registered));

        let moved = Target {
            bridge: std::path::PathBuf::from(
                "/private/var/folders/AppTranslocation/X/d/Pluryprint.app/Contents/MacOS/pluryprint-bridge",
            ),
            ..target
        };
        assert_eq!(
            reconnect_at(&moved).unwrap_err().reason(),
            Some("bridge_translocated")
        );
        assert!(overview_at(&moved).translocated);
    }
}
