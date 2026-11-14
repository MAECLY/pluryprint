//! The browser bridge: a Chrome Native Messaging host for Pluryfact
//! (FR-120..FR-127, ADR-0016).
//!
//! Pluryfact is a browser extension that downloads electronic invoices from
//! Ecuador's tax authority (SRI). It keeps the documents in memory and, when
//! the user asks, sends them here to be printed. The browser starts
//! `pluryprint-bridge` (`src/bin/pluryprint-bridge.rs`) with the caller's
//! origin as its first argument and talks to it over stdin/stdout in
//! length-prefixed JSON messages ([`framing`]). The host:
//!
//! 1. checks the origin against the manifests it is registered with
//!    ([`register`], defence in depth: the browser already did);
//! 2. answers the versioned protocol of [`protocol`] (`hello`, `listPrinters`,
//!    `listProfiles`, `beginJob`, `putFile`, `commitJob`, `abortJob`) through a
//!    [`session::Session`];
//! 3. writes every document it receives, validated ([`validate`]), into a
//!    per-job folder of its own inbox ([`inbox`]) — it never reads a path the
//!    extension chose, and the extension never sends one;
//! 4. on `commitJob`, writes a schema-1 handoff file (`source: "pluryfact"`)
//!    and starts the app with `--invoke=context-menu --files-from <it>`
//!    ([`launch`]), exactly as the Explorer and Finder integrations do, so the
//!    selection reaches a running window through single-instance and is shown
//!    or printed by the same `applySelection` code path.
//!
//! Nothing here opens a socket or talks to the network: the only channel is
//! the pipe the browser opened (security-model.md §6).
//!
//! Every function that touches the disk takes its folder as an argument, so
//! the whole host runs against a temporary directory in tests. The binary
//! resolves the real folders through [`data_dir`], which honours the
//! test-only [`DATA_DIR_ENV`] override.

pub mod framing;
pub mod inbox;
pub mod launch;
pub mod protocol;
pub mod redact;
pub mod register;
pub mod session;
pub mod validate;

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The host name the browser looks up (`chrome.runtime.connectNative`).
/// Lowercase letters, digits, `_` and `.` only, as Chrome requires.
pub const HOST_NAME: &str = "com.maecly.pluryprint";

/// Pluryfact's origin on the Chrome Web Store. The only origin a production
/// manifest lists; others are added with `pluryprint-cli bridge install
/// --extra-origin` for development builds of the extension.
pub const PLURYFACT_ORIGIN: &str = "chrome-extension://ibgodfpcihlddbbfaoooehfgbmahipbj/";

/// The value `beginJob.source` must carry and the handoff's `source`.
pub const SOURCE_PLURYFACT: &str = "pluryfact";

/// **Tests only.** Absolute folder the host uses instead of the app's data
/// directory for its `inbox/` and `handoff/` folders (and the origin check's
/// test manifest, see [`register::installed_origins_in`]). Lets the
/// integration tests run the real binary without touching the developer's
/// data. Not documented to users; a relative value is ignored.
pub const DATA_DIR_ENV: &str = "PLURYPRINT_BRIDGE_DATA_DIR";

/// **Tests only.** When `1`, `commitJob` writes the handoff file and answers
/// `handed-off` without starting the app, so a test can inspect the handoff
/// instead of a window opening on the developer's desktop.
pub const NO_LAUNCH_ENV: &str = "PLURYPRINT_BRIDGE_NO_LAUNCH";

/// How long an inbox folder may stay before the app's startup sweep deletes
/// it (FR-126). Long enough for a queued batch to be printed days later,
/// short enough that the invoices do not pile up on disk.
pub const INBOX_RETENTION: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

/// The folder under which the host keeps `inbox/` and writes `handoff/`.
///
/// The app's data directory (`util::paths::data_dir`), unless the test-only
/// [`DATA_DIR_ENV`] names an absolute folder.
pub fn data_dir() -> PathBuf {
    data_dir_from(std::env::var_os(DATA_DIR_ENV))
}

/// [`data_dir`] as a pure function of the override variable.
pub fn data_dir_from(over: Option<OsString>) -> PathBuf {
    test_data_dir_from(over).unwrap_or_else(crate::util::paths::data_dir)
}

/// The test-only data directory [`DATA_DIR_ENV`] names, when it names an
/// absolute folder; `None` in production. Pure.
pub fn test_data_dir_from(over: Option<OsString>) -> Option<PathBuf> {
    over.map(PathBuf::from).filter(|p| p.is_absolute())
}

/// Where received documents are written: `<data_dir>/inbox`.
pub fn inbox_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("inbox")
}

/// Where the handoff file is written: `<data_dir>/handoff`, the folder the
/// app's context-menu path accepts handoff files from
/// ([`crate::cli::handoff::handoff_dir`]).
pub fn handoff_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("handoff")
}

/// Whether the test-only [`NO_LAUNCH_ENV`] asks not to start the app.
pub fn launch_disabled() -> bool {
    std::env::var_os(NO_LAUNCH_ENV).is_some_and(|v| v == "1")
}

/// Run the host over the given streams until the browser closes stdin.
///
/// `argv` is the process' own (program name first). The return value is the
/// process exit code: 0 when the browser closed the pipe, 1 when the pipe
/// broke or a frame could not be read. Jobs never committed are deleted on
/// the way out ([`session::Session::close`]).
pub fn serve<R: Read, W: Write, H: session::Host>(
    argv: &[OsString],
    input: &mut R,
    output: &mut W,
    host: H,
    allowed: &[String],
) -> i32 {
    let origin = argv.get(1).map(|o| o.to_string_lossy().into_owned());
    let origin_ok = origin
        .as_deref()
        .is_some_and(|o| allowed.iter().any(|a| a == o));
    if !origin_ok {
        tracing::warn!(
            event = "bridge_origin_refused",
            origin = %origin.as_deref().map(redact::origin_for_log).unwrap_or_default()
        );
    }
    let mut session = session::Session::new(host, origin_ok);
    let code = loop {
        match framing::read_message(input) {
            Ok(Some(framing::Frame::Message(bytes))) => {
                let reply = session.handle_bytes(&bytes);
                if let Err(e) = framing::write_message(output, &reply) {
                    tracing::warn!(event = "bridge_write_failed", error = %e);
                    break 1;
                }
            }
            Ok(Some(framing::Frame::Oversized(len))) => {
                let reply = session.oversized(len);
                if let Err(e) = framing::write_message(output, &reply) {
                    tracing::warn!(event = "bridge_write_failed", error = %e);
                    break 1;
                }
            }
            Ok(None) => break 0,
            Err(e) => {
                tracing::warn!(event = "bridge_read_failed", error = %e);
                break 1;
            }
        }
    };
    session.close();
    code
}

/// The binary's whole `main`: real stdin/stdout, the real host, the allowed
/// origins read from the installed manifests. Never writes anything but
/// protocol frames to stdout.
pub fn run_stdio(argv: Vec<OsString>) -> i32 {
    let data = data_dir();
    // Logs go to the app's log file, never stdout (the protocol channel). A
    // test run (data dir overridden) logs nowhere, so it does not write into
    // the developer's real log folder.
    if std::env::var_os(DATA_DIR_ENV).is_none() {
        let _ = crate::logging::init_cli();
    }
    tracing::info!(
        event = "bridge_started",
        version = env!("CARGO_PKG_VERSION")
    );
    // Only a test run (data dir overridden) reads a test manifest from the
    // data dir; in production the origins come from the browsers' manifests
    // alone, so no file in the data dir can widen them.
    let test_dir = test_data_dir_from(std::env::var_os(DATA_DIR_ENV));
    let allowed = register::allowed_origins(test_dir.as_deref());
    let host = session::RealHost::new(data, !launch_disabled());
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let code = serve(&argv, &mut stdin.lock(), &mut stdout.lock(), host, &allowed);
    tracing::info!(event = "bridge_stopped", exit_code = code);
    crate::logging::flush();
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only an absolute override is a test data dir; without one (production)
    /// there is none, so the origin check reads no manifest from the data
    /// dir ([`register::allowed_origins`]).
    #[test]
    fn fr_127_the_test_data_dir_exists_only_when_overridden() {
        assert_eq!(test_data_dir_from(None), None);
        assert_eq!(test_data_dir_from(Some("relative/dir".into())), None);
        let abs = std::env::temp_dir();
        assert_eq!(
            test_data_dir_from(Some(abs.clone().into())),
            Some(abs.clone())
        );
        assert_eq!(data_dir_from(Some(abs.clone().into())), abs);
        assert_eq!(data_dir_from(None), crate::util::paths::data_dir());
    }
}
