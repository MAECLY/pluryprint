//! Round-3 contract on `tauri.conf.json` and the main capability (lane L0).
//!
//! - one version source: Cargo.toml (`tauri.conf.json` has no `version`, so
//!   the updater and `CARGO_PKG_VERSION` can never disagree);
//! - Linux ships as an AppImage and a .deb next to the Windows and macOS
//!   bundles (the ADR that supersedes decision D20); the .deb depends on the
//!   CUPS client tools, installs the file-manager actions and names its
//!   maintainer;
//! - the NSIS installer speaks English and Spanish (FR-093);
//! - the Explorer extension DLLs ship under `resources/shellext/<arch>/`
//!   (directory form, so the per-architecture folders survive bundling);
//! - IPC is granted to the main window only: the render worker's web view has
//!   none.

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn conf() -> serde_json::Value {
    serde_json::from_str(&read("tauri.conf.json")).expect("tauri.conf.json parses")
}

#[test]
fn release_version_has_a_single_source() {
    assert!(
        conf().get("version").is_none(),
        "tauri.conf.json must not pin a version: Cargo.toml is the only source"
    );
}

/// NFR-070: every shipped platform is a bundle target, and a plain
/// `tauri build` on Linux makes the two packages the release ships. The
/// bundler skips the targets of other platforms, so Windows and macOS build
/// what they always did. No .rpm: Fedora users run the AppImage.
#[test]
fn nfr_070_bundle_targets_cover_every_shipped_platform() {
    let conf = conf();
    let targets: Vec<&str> = conf["bundle"]["targets"]
        .as_array()
        .expect("explicit targets")
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    assert_eq!(targets, ["nsis", "app", "dmg", "appimage", "deb"]);
}

/// NFR-070: the .deb. Every print runs the CUPS command-line tools (`lp`,
/// `lpstat`, `lpoptions`, `cancel`), so it depends on cups-client; the IPP
/// tools (`ipptool`, `ippfind`) and the HEIC decoder (`heif-convert`) are
/// recommended, which apt installs by default and a user may skip. tauri
/// adds the WebKitGTK and GTK libraries to Depends on its own, without a
/// version; the binary needs glibc 2.35 (what it is built against) and
/// WebKitGTK 2.42 (wry imports `webkit_cookie_manager_get_all_cookies`, new
/// in 2.41.90, and the binary binds every symbol at start), so Depends says
/// so: an un-updated Ubuntu 22.04 with WebKitGTK 2.36 must not install a
/// package that cannot start.
#[test]
fn nfr_070_deb_depends_on_the_cups_client_tools() {
    let conf = conf();
    let deb = &conf["bundle"]["linux"]["deb"];
    assert_eq!(
        deb["depends"],
        serde_json::json!([
            "cups-client",
            "libc6 (>= 2.35)",
            "libwebkit2gtk-4.1-0 (>= 2.42)"
        ])
    );
    assert_eq!(
        deb["recommends"],
        serde_json::json!(["cups-ipp-utils", "libheif-examples"])
    );
    assert_eq!(deb["section"], "utils");
    // The AppImage does not carry GStreamer: nothing plays audio or video.
    assert_eq!(
        conf["bundle"]["linux"]["appimage"]["bundleMediaFramework"],
        false
    );
}

/// The .deb's `Maintainer` is `Maecly <hola@maecly.com>`. tauri's deb
/// bundler writes the Cargo.toml `authors` there (joined by ", ") and falls
/// back to `bundle.publisher` only when there are none; `bundle.publisher`
/// is also the Windows installer's publisher, which stays `Maecly`. The
/// release workflow checks the built package's field too.
#[test]
fn nfr_070_deb_maintainer_is_maecly() {
    assert_eq!(env!("CARGO_PKG_AUTHORS"), "Maecly <hola@maecly.com>");
    assert_eq!(conf()["bundle"]["publisher"], "Maecly");
    assert!(
        read("../.github/workflows/release.yml")
            .contains(r#"[ "$maintainer" = 'Maecly <hola@maecly.com>' ]"#),
        "release.yml checks the .deb's Maintainer"
    );
}

/// FR-001 on Linux: the .deb's desktop entry carries the MIME types for
/// "Open with Pluryprint", and it installs the Dolphin service menu and the
/// Nemo actions (Print now, Configure print job, Add to print queue) where
/// those file managers look for them system-wide. Every source file exists.
#[test]
fn fr_001_deb_installs_the_file_manager_actions() {
    let conf = conf();
    let deb = &conf["bundle"]["linux"]["deb"];
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(deb["desktopTemplate"], "linux/pluryprint.desktop.hbs");
    let files = deb["files"].as_object().expect("deb.files");
    let wanted = [
        (
            "/usr/share/kio/servicemenus/pluryprint.desktop",
            "linux/kio-servicemenu.desktop",
        ),
        (
            "/usr/share/nemo/actions/pluryprint-print-now.nemo_action",
            "linux/nemo/pluryprint-print-now.nemo_action",
        ),
        (
            "/usr/share/nemo/actions/pluryprint-configure.nemo_action",
            "linux/nemo/pluryprint-configure.nemo_action",
        ),
        (
            "/usr/share/nemo/actions/pluryprint-queue.nemo_action",
            "linux/nemo/pluryprint-queue.nemo_action",
        ),
    ];
    assert_eq!(files.len(), wanted.len(), "{files:?}");
    for (dest, src) in wanted {
        assert_eq!(files[dest], src, "{dest}");
    }
    for src in wanted
        .iter()
        .map(|(_, s)| *s)
        .chain(["linux/pluryprint.desktop.hbs"])
    {
        assert!(root.join(src).is_file(), "{src} is missing");
    }
}

/// The Linux packages install the icon in every hicolor size a desktop asks
/// for (32 to 512), and every icon the bundle names exists.
#[test]
fn nfr_070_linux_icons_cover_the_hicolor_sizes() {
    let conf = conf();
    let icons: Vec<&str> = conf["bundle"]["icon"]
        .as_array()
        .expect("bundle.icon")
        .iter()
        .map(|i| i.as_str().unwrap())
        .collect();
    for png in [
        "icons/32x32.png",
        "icons/64x64.png",
        "icons/128x128.png",
        "icons/128x128@2x.png",
        "icons/icon.png",
    ] {
        assert!(icons.contains(&png), "{png} missing: {icons:?}");
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for icon in icons {
        assert!(root.join(icon).is_file(), "{icon}");
    }
}

/// The package description names every platform it is for.
#[test]
fn nfr_070_long_description_names_linux() {
    let conf = conf();
    let text = conf["bundle"]["longDescription"].as_str().unwrap();
    assert!(text.contains("Windows, macOS and Linux"), "{text}");
}

#[test]
fn fr_093_nsis_installer_is_english_and_spanish() {
    let conf = conf();
    let langs = &conf["bundle"]["windows"]["nsis"]["languages"];
    assert_eq!(langs, &serde_json::json!(["English", "Spanish"]));
}

#[test]
fn fr_001_shell_extension_dlls_ship_per_architecture() {
    let conf = conf();
    assert_eq!(
        conf["bundle"]["resources"]["resources/shellext/"], "resources/shellext/",
        "directory form keeps resources/shellext/<arch>/ intact"
    );
    // The folder exists in a fresh checkout, so a build without the DLLs
    // (every non-Windows build) still finds the resource.
    assert!(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources/shellext/README.md")
        .is_file());
}

#[test]
fn nfr_render_worker_gets_no_ipc() {
    let cap: serde_json::Value =
        serde_json::from_str(&read("capabilities/main.json")).expect("capability parses");
    assert_eq!(cap["windows"], serde_json::json!(["main"]));
}

/// The repository was renamed from `MAECLY/maecly-print` to `MAECLY/pluryprint`
/// on 2026-10-10 (ADR-0011). Installed 0.0.1 builds still reach the old
/// endpoint through GitHub's redirect; every build from 0.0.2 on must name the
/// new slug directly, so the update check never depends on that redirect.
const STABLE_MANIFEST: &str =
    "https://github.com/MAECLY/pluryprint/releases/latest/download/latest.json";
const BETA_MANIFEST: &str =
    "https://github.com/MAECLY/pluryprint/releases/download/channel-beta/latest.json";
const OLD_SLUG: &str = "maecly-print";

#[test]
fn fr_085_updater_endpoint_names_the_renamed_repository() {
    let conf = conf();
    assert_eq!(
        conf["plugins"]["updater"]["endpoints"],
        serde_json::json!([STABLE_MANIFEST])
    );
    let install = read("src/update/install.rs");
    assert!(
        install.contains(&format!("\"{STABLE_MANIFEST}\"")),
        "STABLE_ENDPOINT"
    );
    assert!(
        install.contains(&format!("\"{BETA_MANIFEST}\"")),
        "BETA_ENDPOINT"
    );
    assert!(
        read("../src/lib/repo.ts")
            .contains("export const REPO_URL = 'https://github.com/MAECLY/pluryprint';"),
        "src/lib/repo.ts REPO_URL"
    );
}

#[test]
fn old_repository_slug_is_gone_from_the_update_path() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![
        root.join("tauri.conf.json"),
        root.join("Cargo.toml"),
        root.join("../src/lib/repo.ts"),
    ];
    for dir in ["src/update", "../.github/workflows"] {
        for entry in std::fs::read_dir(root.join(dir)).expect(dir) {
            let path = entry.unwrap().path();
            if path.is_file() {
                files.push(path);
            }
        }
    }
    assert!(files.len() > 5, "scanned too few files: {files:?}");
    for path in files {
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(
            !text.to_ascii_lowercase().contains(OLD_SLUG),
            "{} still names the old repository slug `{OLD_SLUG}`",
            path.display()
        );
    }
}
