//! FR-127 — the browser bridge ships with every bundle, and the Windows
//! installer registers exactly the manifest the bridge's own code would
//! write. Nothing here builds a bundle; the contract is pinned on the
//! sources, like `tauri_conf_contract.rs` and `installer_hooks.rs`.

use std::path::PathBuf;

use pluryprint_lib::bridge::register::{self, Browser, Manifest};
use pluryprint_lib::bridge::{HOST_NAME, PLURYFACT_ORIGIN};

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(rel)
}

fn read(rel: &str) -> String {
    let p = repo(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The bridge is a cargo binary of the app crate. Tauri's bundler installs
/// every binary of the crate next to the app (`pluryprint-cli` ships the same
/// way): `%LOCALAPPDATA%\Pluryprint\` on Windows,
/// `Pluryprint.app/Contents/MacOS/` on macOS, `/usr/bin` in the .deb. So the
/// contract is that it stays a binary target and nothing narrows the set.
#[test]
fn fr_127_the_bridge_is_a_bundled_binary_next_to_the_cli() {
    assert!(repo("src-tauri/src/bin/pluryprint-bridge.rs").is_file());
    assert!(repo("src-tauri/src/bin/pluryprint-cli.rs").is_file());
    // Cargo built it for this very test run (the variable only exists for a
    // binary target of this package).
    assert!(std::path::Path::new(env!("CARGO_BIN_EXE_pluryprint-bridge")).is_file());
    let cargo = read("src-tauri/Cargo.toml");
    assert!(
        !cargo.contains("autobins = false") && !cargo.contains("[[bin]]"),
        "src/bin/* must stay auto-discovered, so the bridge is bundled"
    );
    let conf: serde_json::Value = serde_json::from_str(&read("src-tauri/tauri.conf.json")).unwrap();
    assert!(
        conf["bundle"].get("externalBin").is_none(),
        "the bridge is a crate binary, not a sidecar"
    );
    // A console program: no `windows_subsystem` attribute (Native Messaging
    // needs real standard handles).
    let main = read("src-tauri/src/bin/pluryprint-bridge.rs");
    assert!(!main
        .lines()
        .any(|l| l.trim_start().starts_with("#!") && l.contains("windows_subsystem")));
}

/// The release workflow fails a bundle that lacks the bridge, on every OS.
#[test]
fn fr_127_the_release_checks_every_bundle_carries_the_bridge() {
    let wf = read(".github/workflows/release.yml");
    assert!(wf.contains("foreach ($f in 'pluryprint-cli.exe', 'pluryprint-bridge.exe')"));
    assert!(wf.contains("for f in pluryprint pluryprint-cli pluryprint-bridge; do"));
    assert!(wf.contains("usr/bin/pluryprint-bridge"));
}

/// Concatenate the `FileWrite $R9 '…'` lines of the installer's
/// manifest-writing macro into the file it produces.
fn nsis_manifest(nsh: &str) -> String {
    let body = nsh
        .split("!macro PLURYPRINT_REGISTER_BRIDGE")
        .nth(1)
        .and_then(|r| r.split("!macroend").next())
        .expect("the registration macro");
    let define = |name: &str| -> String {
        let prefix = format!("!define {name} \"");
        nsh.lines()
            .find_map(|l| l.strip_prefix(&prefix))
            .and_then(|r| r.strip_suffix('"'))
            .unwrap_or_else(|| panic!("no {name}"))
            .to_string()
    };
    let mut out = String::new();
    for line in body.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("FileWrite $R9 '") {
            out.push_str(rest.strip_suffix('\'').expect("single-quoted"));
        }
    }
    out.replace("$\\r$\\n", "\r\n")
        .replace(
            "${PLURYPRINT_BRIDGE_HOST}",
            &define("PLURYPRINT_BRIDGE_HOST"),
        )
        .replace("${PLURYPRINT_BRIDGE_EXE}", &define("PLURYPRINT_BRIDGE_EXE"))
        .replace(
            "${PLURYPRINT_PLURYFACT_ORIGIN}",
            &define("PLURYPRINT_PLURYFACT_ORIGIN"),
        )
}

/// The manifest the installer writes is, field for field, the one
/// `bridge::register` writes on Windows: the host name, the bridge's file
/// name relative to the manifest, `stdio`, and Pluryfact's origin only.
#[test]
fn fr_127_the_installer_writes_the_manifest_the_bridge_expects() {
    let nsh = read("src-tauri/windows/installer-hooks.nsh");
    let text = nsis_manifest(&nsh);
    let written: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    let expected = serde_json::to_value(Manifest::new(
        "pluryprint-bridge.exe".to_string(),
        register::origins_with(&[]),
    ))
    .unwrap();
    assert_eq!(written, expected);
    assert_eq!(
        written["allowed_origins"],
        serde_json::json!([PLURYFACT_ORIGIN])
    );
    assert!(nsh.contains(&format!("!define PLURYPRINT_BRIDGE_HOST \"{HOST_NAME}\"")));
    // The manifest sits next to the bridge, as register::windows_manifest_path.
    assert!(nsh.contains("FileOpen $R9 \"$INSTDIR\\${PLURYPRINT_BRIDGE_HOST}.json\" w"));
}

/// The installer writes the key of every browser vendor the bridge knows
/// (Brave reads Chrome's), under HKCU, pointing at the manifest; the
/// uninstaller deletes each key and the manifest.
#[test]
fn fr_127_the_installer_writes_and_the_uninstaller_removes_the_keys() {
    let nsh = read("src-tauri/windows/installer-hooks.nsh");
    let key_of = |define: &str| -> String {
        let prefix = format!("!define {define} \"");
        nsh.lines()
            .find_map(|l| l.strip_prefix(&prefix))
            .and_then(|r| r.strip_suffix('"'))
            .unwrap()
            .replace("${PLURYPRINT_BRIDGE_HOST}", HOST_NAME)
    };
    let keys = [
        ("PLURY_NMH_CHROME", Browser::Chrome),
        ("PLURY_NMH_EDGE", Browser::Edge),
        ("PLURY_NMH_CHROMIUM", Browser::Chromium),
    ];
    for (define, browser) in keys {
        assert_eq!(key_of(define), register::windows_registry_key(browser));
    }
    assert_eq!(
        register::windows_registry_key(Browser::Brave),
        register::windows_registry_key(Browser::Chrome)
    );

    let post = nsh
        .split("!macro NSIS_HOOK_POSTINSTALL")
        .nth(1)
        .and_then(|r| r.split("!macroend").next())
        .unwrap();
    assert!(post.contains("!insertmacro PLURYPRINT_REGISTER_BRIDGE"));
    let reg = nsh
        .split("!macro PLURYPRINT_REGISTER_BRIDGE")
        .nth(1)
        .and_then(|r| r.split("!macroend").next())
        .unwrap();
    let un = nsh
        .split("!macro PLURYPRINT_UNREGISTER_BRIDGE")
        .nth(1)
        .and_then(|r| r.split("!macroend").next())
        .unwrap();
    for (define, _) in keys {
        assert!(
            reg.contains(&format!(
                "WriteRegStr HKCU \"${{{define}}}\" \"\" \"$INSTDIR\\${{PLURYPRINT_BRIDGE_HOST}}.json\""
            )),
            "install writes {define}"
        );
        assert!(
            un.contains(&format!("DeleteRegKey HKCU \"${{{define}}}\"")),
            "uninstall deletes {define}"
        );
    }
    assert!(un.contains("Delete \"$INSTDIR\\${PLURYPRINT_BRIDGE_HOST}.json\""));
    let pre_un = nsh
        .split("!macro NSIS_HOOK_PREUNINSTALL")
        .nth(1)
        .and_then(|r| r.split("!macroend").next())
        .unwrap();
    assert!(pre_un.contains("!insertmacro PLURYPRINT_UNREGISTER_BRIDGE"));
    // A setup without the bridge registers nothing.
    assert!(reg.contains("${If} ${FileExists} \"$INSTDIR\\${PLURYPRINT_BRIDGE_EXE}\""));
    assert!(reg.contains("Push $R9") && reg.contains("Pop $R9"));
}
