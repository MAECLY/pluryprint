//! The release pipeline's contract (FR-080..086, NFR-006, NFR-022, NFR-027,
//! NFR-070), checked from the files themselves so a reviewer does not have to
//! run a release to see it broken:
//!
//! - `.github/workflows/release.yml` builds the four targets with no write
//!   token and no secret, signs the update artifacts in a job that builds
//!   nothing, and only then uploads a draft: it refuses to run without the
//!   updater signing secret, names every asset for the computer it is for,
//!   verifies signatures, writes SHA256SUMS and attaches the Corresponding
//!   Source;
//! - every workflow pins its actions to a commit, keeps no git credentials
//!   and asks for the least permission it needs;
//! - `.github/workflows/release-channels.yml` moves the beta manifest forward
//!   only;
//! - `.github/workflows/landing.yml` redeploys the landing, which lives in
//!   MAECLY/pluryprint-landing, through a Vercel Deploy Hook and no token;
//! - `scripts/release/release_tools.py` implements those rules (exercised
//!   here where a `python3` exists);
//! - `scripts/bump-version.sh` keeps the version sources in step, the
//!   Explorer extension's included.

use std::path::PathBuf;
use std::process::Command;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(rel: &str) -> String {
    let p = repo().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn release_yml() -> String {
    read(".github/workflows/release.yml")
}

/// Every workflow file, as (file name, text).
fn workflows() -> Vec<(String, String)> {
    let dir = repo().join(".github/workflows");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "yml" || x == "yaml"))
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(&p).unwrap(),
            )
        })
        .collect();
    out.sort();
    assert!(out.len() >= 4, "{out:?}");
    out
}

/// Whether `line` opens a job: a key at two spaces of indentation.
fn is_job_key(line: &str) -> bool {
    line.strip_prefix("  ").is_some_and(|key| {
        key.ends_with(':')
            && key
                .trim_end_matches(':')
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            && !key.starts_with(' ')
            && key.len() > 1
    })
}

/// The text of the job named `name`, from its key to the next job's.
fn job(workflow: &str, name: &str) -> String {
    let jobs = workflow
        .split_once("\njobs:\n")
        .unwrap_or_else(|| panic!("no jobs"))
        .1;
    let key = format!("  {name}:");
    let mut lines = jobs.lines().skip_while(|l| *l != key);
    let first = lines.next().unwrap_or_else(|| panic!("no job {name}"));
    let mut out = vec![first];
    out.extend(lines.take_while(|l| !is_job_key(l)));
    out.join("\n")
}

/// The names of every job of `workflow`.
fn job_names(workflow: &str) -> Vec<String> {
    workflow
        .split_once("\njobs:\n")
        .unwrap_or_else(|| panic!("no jobs"))
        .1
        .lines()
        .filter(|l| is_job_key(l))
        .map(|l| l.trim().trim_end_matches(':').to_string())
        .collect()
}

/// The steps of a job's text, each from its `- ` line to the next one's.
fn steps(job: &str) -> Vec<String> {
    let mut out: Vec<Vec<&str>> = Vec::new();
    let mut in_steps = false;
    for line in job.lines() {
        if line == "    steps:" {
            in_steps = true;
            continue;
        }
        if !in_steps {
            continue;
        }
        if line.starts_with("      - ") {
            out.push(vec![line]);
        } else if let Some(step) = out.last_mut() {
            step.push(line);
        }
    }
    out.into_iter().map(|s| s.join("\n")).collect()
}

/// NFR-022: a tag build stops before building anything when the updater
/// signing key is not configured, and every build waits for that check.
#[test]
fn nfr_022_release_requires_signature_secret() {
    let wf = release_yml();
    let guard = job(&wf, "guard");
    assert!(
        guard.contains("secrets.TAURI_SIGNING_PRIVATE_KEY"),
        "{guard}"
    );
    assert!(
        guard.contains(r#"if [ -z "$SIGNING_KEY" ]; then"#),
        "{guard}"
    );
    assert!(guard.contains("exit 1"), "{guard}");
    let build = job(&wf, "build");
    assert!(build.contains("needs: [guard]"), "{build}");
    assert!(
        build.contains("needs.guard.result != 'failure'"),
        "a failed guard must stop the builds: {build}"
    );
    // The key reaches the sign job, which signs the update artifacts.
    let sign = job(&wf, "sign");
    assert!(sign.contains("needs: [build]"), "{sign}");
    assert!(sign.contains("needs.build.result == 'success'"), "{sign}");
    assert!(sign.contains("TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}"));
    assert!(sign.contains(
        "TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}"
    ));
    assert!(
        sign.contains("pnpm --silent tauri signer sign --app-version \"$version\" \"$f\""),
        "{sign}"
    );
    // The signatures are verified against the key the app carries, and a
    // tampered artifact must fail.
    let publish = job(&wf, "publish");
    assert!(publish.contains("needs: [build, sign]"), "{publish}");
    assert!(publish.contains("minisign -V -p updater.pub"), "{publish}");
    assert!(
        publish.contains("['plugins']['updater']['pubkey']"),
        "{publish}"
    );
    assert!(
        publish.contains("a tampered artifact verified"),
        "{publish}"
    );
    // Each platform is paired with its file through the asset listing, and
    // the workings stay out of assets/, which SHA256SUMS covers.
    assert!(publish.contains("release_tools.py extract-signatures"));
    assert!(publish.contains("--out sigcheck"));
}

/// NFR-022: the release key is in the environment of exactly one step, in a
/// job that compiles nothing. The builds run every crate's build script and
/// every proc macro, so they get no secret and sign with a throwaway key,
/// and none of their signatures is published.
#[test]
fn nfr_022_release_signing_key_reaches_one_step_that_builds_nothing() {
    let wf = release_yml();
    for name in job_names(&wf) {
        let text = job(&wf, &name);
        let with_key: Vec<String> = steps(&text)
            .into_iter()
            .filter(|s| s.contains("secrets.TAURI_SIGNING_PRIVATE_KEY"))
            .collect();
        match name.as_str() {
            // Only tests that it is set; nothing is built or signed there.
            "guard" => {
                assert_eq!(with_key.len(), 1, "{text}");
                assert!(with_key[0].contains("Signing key must be configured"));
            }
            "sign" => {
                assert_eq!(
                    with_key.len(),
                    1,
                    "the key is in more than one step: {text}"
                );
                assert!(with_key[0].contains("name: Sign with the release key"));
                assert!(
                    !text.contains("tauri build") && !text.contains("cargo "),
                    "the sign job must not build: {text}"
                );
                // No step-level env leaks to the job.
                assert!(!text.contains("\n    env:"), "{text}");
            }
            _ => assert!(with_key.is_empty(), "job {name} sees the release key"),
        }
    }
    let build = job(&wf, "build");
    assert!(
        !build.contains("secrets."),
        "the builds read no secret: {build}"
    );
    // The build's key is generated on the spot and deleted.
    assert!(build.contains("pnpm tauri signer generate --ci --force -p \"$pass\" -w \"$key\""));
    assert!(build.contains("rm -f \"$key\" \"$key.pub\""));
    // ...and its signatures never reach the release: the staging step copies
    // the files without them (release_tools.py stage-bundle), and the sign
    // job writes the only .sig files the draft gets.
    assert!(build.contains("release_tools.py stage-bundle"));
    assert!(build.contains("name: unsigned-${{ matrix.label }}"));
}

/// NFR-022: only the job that uploads the draft can write to the
/// repository, and it runs no third-party build code. The builds get a
/// read-only token, and no git credentials stay on any runner.
#[test]
fn nfr_022_release_jobs_have_least_privilege() {
    let wf = release_yml();
    assert!(
        wf.contains("\npermissions: {}\n"),
        "release.yml must grant nothing by default"
    );
    for name in job_names(&wf) {
        let text = job(&wf, &name);
        let write = text.contains("contents: write");
        assert_eq!(
            write,
            name == "publish",
            "job {name}: contents: write is for the publish job only"
        );
        assert!(
            text.contains("    permissions:\n"),
            "job {name} must state its permissions"
        );
        if name != "publish" {
            assert!(text.contains("      contents: read"), "{name}");
        }
    }
    let publish = job(&wf, "publish");
    for forbidden in ["pnpm install", "cargo ", "tauri build", "npm "] {
        assert!(
            !publish.contains(forbidden),
            "the publish job holds a write token and must not run {forbidden}"
        );
    }
    // No dependency or build cache can reach a release build.
    assert!(!wf.contains("actions/cache@"), "release.yml uses a cache");
    assert!(!wf.contains("cache: pnpm"), "release.yml uses a cache");
    // setup-node v5+ also caches on its own when package.json names a
    // package manager it covers, so every release setup-node turns that off
    // by name instead of relying on today's npm-only default.
    let lines: Vec<&str> = wf.lines().collect();
    let mut setup_nodes = 0;
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("uses: actions/setup-node@") {
            continue;
        }
        setup_nodes += 1;
        let with: String = lines[i + 1..(i + 5).min(lines.len())].join("\n");
        assert!(
            with.contains("package-manager-cache: false"),
            "release.yml:{}: setup-node may restore a package-manager cache",
            i + 1
        );
    }
    assert!(setup_nodes > 0, "release.yml no longer sets up Node");
    // No package install script runs in a release.
    for line in wf.lines().filter(|l| l.contains("pnpm install")) {
        assert!(
            line.contains("pnpm install --frozen-lockfile --ignore-scripts"),
            "{line}"
        );
    }
}

/// Every action of every workflow is pinned to a full commit SHA with the
/// version it was in a comment, every checkout drops its credentials, and
/// every workflow states its permissions at the top. A moved tag, or a
/// compromised action, cannot then run code next to a token or a secret.
#[test]
fn nfr_022_every_workflow_action_is_pinned_and_checkouts_keep_no_credentials() {
    for (file, text) in workflows() {
        assert!(
            text.contains("\npermissions:"),
            "{file} does not state its permissions"
        );
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let Some((_, used)) = line.split_once("uses: ") else {
                continue;
            };
            if used.starts_with("./") {
                continue;
            }
            let (action, rest) = used
                .split_once('@')
                .unwrap_or_else(|| panic!("{file}:{}: {line}", i + 1));
            let (sha, comment) = rest.split_once(" # ").unwrap_or((rest, ""));
            assert!(
                sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
                "{file}:{}: {action} is not pinned to a commit: {line}",
                i + 1
            );
            assert!(
                !comment.trim().is_empty(),
                "{file}:{}: say which version {sha} is",
                i + 1
            );
            if action == "actions/checkout" {
                let next: String = lines[i + 1..(i + 4).min(lines.len())].join("\n");
                assert!(
                    next.contains("persist-credentials: false"),
                    "{file}:{}: checkout keeps the token in .git/config",
                    i + 1
                );
            }
        }
    }
}

/// The version a release is tagged with is the version every source carries:
/// Cargo.toml (the only one the app reads), package.json (kept equal) and no
/// `version` in tauri.conf.json.
#[test]
fn release_version_sources_agree() {
    let cargo = read("src-tauri/Cargo.toml");
    let cargo_version = cargo
        .lines()
        .find(|l| l.starts_with("version = "))
        .and_then(|l| l.split('"').nth(1))
        .expect("Cargo.toml [package] version");
    assert_eq!(cargo_version, env!("CARGO_PKG_VERSION"));

    let pkg: serde_json::Value = serde_json::from_str(&read("package.json")).unwrap();
    assert_eq!(
        pkg["version"], cargo_version,
        "package.json and Cargo.toml disagree"
    );

    let conf: serde_json::Value = serde_json::from_str(&read("src-tauri/tauri.conf.json")).unwrap();
    assert!(conf.get("version").is_none());

    // The guard compares the tag with all of them.
    let guard = job(&release_yml(), "guard");
    for needle in [
        "src-tauri/Cargo.toml",
        "package.json",
        "src-tauri/tauri.conf.json",
        "shell-ext-win/Cargo.toml",
        "shell-ext-win/res/shellext.rc",
    ] {
        assert!(guard.contains(needle), "guard does not read {needle}");
    }
}

/// The Explorer extension ships in the same installer as the app, so it
/// carries the app's version: its crate version and the VERSIONINFO Windows
/// shows for shell_ext_win.dll (X.Y.Z.0, no pre-release suffix). 0.0.2 shipped
/// a DLL that still said 0.0.1.0 (#63).
#[test]
fn release_explorer_extension_version_is_the_app_version() {
    let app = env!("CARGO_PKG_VERSION");
    let ext = read("shell-ext-win/Cargo.toml");
    let ext_version = ext
        .lines()
        .find(|l| l.starts_with("version = "))
        .and_then(|l| l.split('"').nth(1))
        .expect("shell-ext-win/Cargo.toml [package] version");
    assert_eq!(ext_version, app, "shell-ext-win/Cargo.toml");

    let lock = read("shell-ext-win/Cargo.lock");
    assert!(
        lock.contains(&format!(
            "name = \"pluryprint-shellext\"\nversion = \"{app}\""
        )),
        "shell-ext-win/Cargo.lock"
    );

    let core = app.split('-').next().unwrap();
    let dotted = format!("{core}.0");
    let commas = dotted.replace('.', ",");
    let rc = read("shell-ext-win/res/shellext.rc");
    let field = |key: &str| {
        rc.lines()
            .map(str::trim)
            .find(|l| l.starts_with(key))
            .unwrap_or_else(|| panic!("no {key} in shellext.rc"))
            .to_string()
    };
    assert_eq!(field("FILEVERSION"), format!("FILEVERSION     {commas}"));
    assert_eq!(field("PRODUCTVERSION"), format!("PRODUCTVERSION  {commas}"));
    for key in ["VALUE \"FileVersion\"", "VALUE \"ProductVersion\""] {
        assert!(
            field(key).ends_with(&format!("\"{dotted}\"")),
            "{key} is not {dotted}"
        );
    }
}

/// The four labels, in matrix order, and the latest.json key each serves.
const LABELS: [(&str, &str); 4] = [
    ("apple_silicon", "darwin-aarch64"),
    ("apple_intel", "darwin-x86_64"),
    ("windows_x64", "windows-x86_64"),
    ("windows_arm64", "windows-aarch64"),
];

/// NFR-070 and decision D2: four builds, each named for the computer it is
/// for (Versorium's layout), Windows as NSIS only.
#[test]
fn nfr_070_release_builds_every_platform_with_its_own_asset_names() {
    let build = job(&release_yml(), "build");
    for (args, (label, _)) in [
        "--target aarch64-apple-darwin --bundles app,dmg",
        "--target x86_64-apple-darwin --bundles app,dmg",
        "--target x86_64-pc-windows-msvc --bundles nsis",
        "--target aarch64-pc-windows-msvc --bundles nsis",
    ]
    .into_iter()
    .zip(LABELS)
    {
        assert!(build.contains(&format!("args: '{args}'")), "missing {args}");
        let at = build.find(&format!("args: '{args}'")).unwrap();
        assert!(
            build[at..].contains(&format!("label: {label}")),
            "missing label {label}"
        );
    }
    assert!(build.contains("pnpm tauri build $ARGS --ci"));
    assert!(build.contains("--label \"$LABEL\""));
    // The labels and the manifest keys are the release tools' own table.
    let tools = read("scripts/release/release_tools.py");
    for (label, key) in LABELS {
        assert!(
            tools.contains(&format!("\"{label}\": (\"{key}\"")),
            "release_tools.py does not map {label} to {key}"
        );
    }
    // A draft, never published by the workflow.
    let publish = job(&release_yml(), "publish");
    assert!(publish.contains("gh release create \"$GITHUB_REF_NAME\" --repo \"$REPO\" --draft"));
    assert!(publish.contains("is already published"));
    assert!(!release_yml().contains("--draft=false"));
}

/// Every name a build stages is distinct across the release: the same
/// extension in two builds must not render to one file name, or one build's
/// upload silently replaces the other's (`gh release upload --clobber`).
#[test]
fn release_asset_names_never_collide() {
    let exts = [".dmg", ".app.tar.gz", ".app.tar.gz.sig", ".exe", ".exe.sig"];
    let mut seen = std::collections::HashSet::new();
    for (label, _) in LABELS {
        for ext in exts {
            let name = format!("Pluryprint_1.2.3_{label}{ext}");
            assert!(seen.insert(name.clone()), "two builds would upload {name}");
        }
    }
    // Each build uploads under its own artifact name too.
    assert!(job(&release_yml(), "build").contains("name: unsigned-${{ matrix.label }}"));
}

/// Decision D2: no certificates, so the release notes say how to get past
/// Gatekeeper and SmartScreen, in English and Spanish.
#[test]
fn d2_release_notes_explain_gatekeeper_and_smartscreen_in_en_and_es() {
    let notes = read(".github/release-notes.md");
    for needle in [
        "xattr -dr com.apple.quarantine /Applications/Pluryprint.app",
        "**Open Anyway**",
        "**Abrir igualmente**",
        "SHA256SUMS --ignore-missing",
        "Smart App Control",
        "**More info**, then **Run anyway**",
        "**Más información** y después **Ejecutar de todas formas**",
        "## English",
        "## Español",
        "apple_silicon",
        "apple_intel",
        "windows_x64",
        "windows_arm64",
    ] {
        assert!(notes.contains(needle), "release notes lack {needle:?}");
    }
    let publish = job(&release_yml(), "publish");
    assert!(publish.contains(".github/release-notes.md > release-notes.md"));
    assert!(publish.contains("--notes-file release-notes.md"));
}

/// FR-085: the manifest is complete for the four platforms the app updates
/// on before anyone can publish, with URLs the app can download, and
/// checksums of every file.
#[test]
fn fr_085_publish_requires_every_platform_and_writes_sha256sums() {
    let publish = job(&release_yml(), "publish");
    for key in [
        "darwin-aarch64",
        "darwin-x86_64",
        "windows-x86_64",
        "windows-aarch64",
    ] {
        assert!(
            publish.contains(&format!("--require {key}")),
            "publish does not require {key}"
        );
    }
    assert!(publish.contains("needs.sign.result == 'success'"));
    assert!(publish.contains("release_tools.py make-manifest"));
    assert!(publish.contains("sha256sum -- * > ../SHA256SUMS"));
    assert!(publish.contains("gh release upload \"$GITHUB_REF_NAME\" SHA256SUMS latest.json"));
}

/// FR-082/NFR-022: the app's updater has `requireSignedVersion` on, so every
/// update signature must carry the release's version. The sign job binds
/// each one to the tag, and the draft is not written until every required
/// platform's signature says so.
#[test]
fn fr_082_update_signatures_are_bound_to_the_release_version() {
    let wf = release_yml();
    let sign = job(&wf, "sign");
    assert!(
        sign.contains("tauri signer sign --app-version \"$version\" \"$f\""),
        "{sign}"
    );
    assert!(sign.contains("version=\"${GITHUB_REF_NAME#v}\""), "{sign}");
    let publish = job(&wf, "publish");
    assert!(
        publish.contains("--signed-version \"${GITHUB_REF_NAME#v}\""),
        "{publish}"
    );
}

/// FR-082: `check-manifest --signed-version` reads the version from each
/// signature's trusted comment, in the form `tauri signer sign --app-version`
/// writes it, and refuses a signature with none or with another version.
#[test]
fn fr_082_check_manifest_requires_signatures_bound_to_the_version() {
    if python().is_none() {
        eprintln!("no python on this machine; release_tools.py not exercised");
        return;
    }
    use base64::Engine as _;
    let sig = |trusted: &str| {
        base64::engine::general_purpose::STANDARD.encode(format!(
            "untrusted comment: signature from tauri secret key\nRUS6c2ln\n\
             trusted comment: {trusted}\nZ2xvYmFs\n"
        ))
    };
    let dir = tempfile::tempdir().unwrap();
    let check = |signature: String, wanted: &str| {
        let manifest = dir.path().join("latest.json");
        let entry = serde_json::json!({ "url": "https://x/1", "signature": signature });
        std::fs::write(
            &manifest,
            serde_json::json!({ "version": "0.2.0", "platforms": { "windows-x86_64": entry } })
                .to_string(),
        )
        .unwrap();
        tools(&[
            "check-manifest",
            "--manifest",
            &manifest.to_string_lossy(),
            "--version",
            "0.2.0",
            "--signed-version",
            wanted,
            "--require",
            "windows-x86_64",
        ])
        .unwrap()
    };

    let bound = sig("timestamp:1791667274\tfile:Pluryprint_0.2.0_windows_x64.exe\tversion:0.2.0");
    let ok = check(bound.clone(), "0.2.0");
    assert!(ok.status.success(), "{ok:?}");
    // A leading `v` on the tag is the same version.
    assert!(check(bound.clone(), "v0.2.0").status.success());

    let other = check(bound, "0.3.0");
    assert_eq!(other.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&other.stderr).contains("bound to '0.2.0'"));

    // Signed by a CLI (or by hand) without --app-version.
    let unbound = check(sig("timestamp:1791667275\tfile:x.exe"), "0.2.0");
    assert_eq!(unbound.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&unbound.stderr).contains("not bound to a version"));

    // Not a signature at all.
    let garbage = check("%%%".into(), "0.2.0");
    assert_eq!(garbage.status.code(), Some(1));
}

/// FR-085/086: publishing a release moves the beta manifest, forward only.
#[test]
fn fr_086_beta_pointer_moves_forward_only() {
    let wf = read(".github/workflows/release-channels.yml");
    assert!(wf.contains("types: [published]"));
    assert!(wf.contains("BETA_TAG: channel-beta"));
    assert!(wf.contains("release_tools.py semver-gt \"$new_version\" \"$current_version\""));
    assert!(wf.contains("gh release upload \"$BETA_TAG\" new/latest.json"));
    // The tag reaches the shell through the environment, never spliced in.
    assert!(wf.contains("TAG: ${{ github.event.release.tag_name }}"));
    assert!(!wf.contains("run: ${{"));
}

/// Publishing a release redeploys the landing, which lives in
/// MAECLY/pluryprint-landing and is built by Vercel: `landing.yml` only calls
/// the Vercel Deploy Hook. It holds no token, checks nothing out, keeps the
/// hook's URL out of its commands and fails when the hook does.
#[test]
fn landing_redeploys_on_every_published_release() {
    let wf = read(".github/workflows/landing.yml");
    let on = wf
        .split_once("\non:\n")
        .unwrap()
        .1
        .split_once("\npermissions:")
        .unwrap()
        .0;
    assert!(on.contains("  release:\n    types: [published]\n"), "{on}");
    assert!(on.contains("  workflow_dispatch:"), "{on}");
    assert!(
        !on.contains("push:") && !on.contains("pull_request"),
        "{on}"
    );

    // No token at all: the top level grants nothing and no job asks again.
    assert!(wf.contains("\npermissions: {}\n"), "{wf}");
    assert_eq!(wf.matches("permissions:").count(), 1, "{wf}");
    assert!(!wf.contains("actions/checkout"), "{wf}");
    assert!(!wf.contains("github.token") && !wf.contains("GITHUB_TOKEN:"));

    assert_eq!(job_names(&wf), ["redeploy"]);
    let redeploy = job(&wf, "redeploy");
    // The hook's URL reaches the shell through the environment, never
    // spliced into a command, and an empty secret fails the run.
    assert!(
        redeploy.contains("LANDING_DEPLOY_HOOK: ${{ secrets.LANDING_DEPLOY_HOOK }}"),
        "{redeploy}"
    );
    assert_eq!(wf.matches("${{ secrets.").count(), 1, "{wf}");
    let run = &redeploy[redeploy.find("run: |").expect("a run step")..];
    assert!(!run.contains("${{"), "{run}");
    assert!(run.contains("if [ -z \"$LANDING_DEPLOY_HOOK\" ]"), "{run}");
    assert!(run.contains("::error::"), "{run}");
    assert!(
        run.contains("curl --fail-with-body --silent --show-error --retry 3"),
        "{run}"
    );
    assert!(
        run.contains("--request POST \"$LANDING_DEPLOY_HOOK\""),
        "{run}"
    );
    assert!(!run.contains("echo \"$LANDING_DEPLOY_HOOK"), "{run}");
    // Nothing turns a failed deploy into a green run.
    for bypass in ["continue-on-error", "|| true", "|| :", "--insecure", " -k "] {
        assert!(!wf.contains(bypass), "{bypass}");
    }
}

/// AGPL-3.0 and GPL-3.0 section 6: every release carries the source of
/// Pluryprint and of the SumatraPDF build its installers contain.
#[test]
fn nfr_027_releases_attach_the_corresponding_source() {
    let publish = job(&release_yml(), "publish");
    assert!(publish.contains("git archive --format=tar.gz"));
    assert!(publish.contains("pluryprint-$version-source.tar.gz"));
    assert!(publish.contains("SumatraPDF-$sumatra_version-source.tar.gz"));
    // The commit comes from the fetch script, so the two cannot drift.
    assert!(publish.contains("SOURCE_COMMIT"));
    assert!(publish.contains("scripts/fetch-sumatrapdf.sh"));
    // And the Windows builds really fetch SumatraPDF and the DLLs.
    let build = job(&release_yml(), "build");
    assert!(build.contains("./scripts/fetch-sumatrapdf.ps1 -Arch $env:ARCH"));
    assert!(build.contains("./scripts/build-shell-ext.ps1 -Arch all"));
}

/// NFR-070: the x64 and ARM64 installers each carry both Explorer DLLs, so
/// an x64 installer run emulated on Windows on ARM still registers a DLL
/// Explorer can load, and both DLLs export the COM entry points.
#[test]
fn nfr_070_windows_release_builds_and_checks_both_explorer_dlls() {
    let release = job(&release_yml(), "build");
    assert!(release.contains("./scripts/build-shell-ext.ps1 -Arch all"));
    // The release's Windows toolchain has both targets, for the DLLs.
    assert!(release.contains("'x86_64-pc-windows-msvc aarch64-pc-windows-msvc'"));
    // Both installers are checked for both DLLs.
    for dll in [
        "shellext/x64/shell_ext_win.dll",
        "shellext/arm64/shell_ext_win.dll",
    ] {
        assert!(release.contains(dll), "release does not check {dll}");
    }
    // The DLLs that ship are checked for the exports Explorer calls, with a
    // test that answers yes or no (an array `-notmatch` filters instead).
    assert!(release.contains("Explorer DLLs export the COM entry points"));
    assert!(release.contains("foreach ($arch in 'x64', 'arm64')"));
    assert!(release.contains(r#"(& $dumpbin /EXPORTS $dll) -join "`n""#));
    for symbol in ["DllGetClassObject", "DllCanUnloadNow"] {
        assert!(release.contains(symbol), "release does not check {symbol}");
    }
}

/// NFR-006: the installer size is measured on every Windows release build;
/// CI builds no installer.
#[test]
fn nfr_006_installer_size_is_measured_against_the_budget() {
    let build = job(&release_yml(), "build");
    assert!(build.contains("Installer size budget (NFR-006)"));
    assert!(build.contains("$budget = 20MB"));
    let ci = read(".github/workflows/ci.yml");
    assert!(!ci.contains("tauri build"), "ci.yml builds no bundle");
}

/// CI is the lean pull-request gate: frontend lint, typecheck, format and
/// tests, and the Rust backend's fmt, clippy and full test run on both
/// shipped platforms. Coverage, bundles, smoke launches and advisory lanes
/// are not part of it.
#[test]
fn ci_is_the_lean_pull_request_gate() {
    let ci = read(".github/workflows/ci.yml");
    assert_eq!(job_names(&ci), ["frontend", "backend"]);
    assert!(ci.contains("\non:\n  pull_request:\n"), "{ci}");
    assert!(ci.contains("  push:\n    branches: [main]\n"), "{ci}");
    assert!(ci.contains("  workflow_dispatch:\n"), "{ci}");
    assert!(!ci.contains("schedule:"), "ci.yml runs on a schedule");
    assert!(ci.contains("cancel-in-progress: true"));

    let frontend = job(&ci, "frontend");
    for cmd in [
        "pnpm install --frozen-lockfile",
        "pnpm lint",
        "pnpm typecheck",
        "pnpm format:check",
        "pnpm test",
        "pnpm audit --prod --audit-level high",
    ] {
        assert!(frontend.contains(cmd), "frontend does not run {cmd}");
    }

    let backend = job(&ci, "backend");
    assert!(backend.contains("os: [windows-latest, macos-latest]"));
    assert!(backend.contains("PLURYPRINT_SKIP_MIGRATION: '1'"));
    for cmd in [
        "cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check",
        "cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings",
        "cargo clippy --manifest-path shell-ext-win/Cargo.toml --all-targets -- -D warnings",
        "cargo test --manifest-path shell-ext-win/Cargo.toml",
    ] {
        assert!(backend.contains(cmd), "backend does not run {cmd}");
    }
    // The full run is what keeps src/ipc/bindings.ts in sync: no filter.
    assert!(backend.contains("run: cargo test --manifest-path src-tauri/Cargo.toml\n"));

    // Both locked Rust trees are audited, with a pinned cargo-audit, and
    // only the ignores documented in src-tauri/.cargo/audit.toml apply (no
    // `--ignore` on the command line, no warnings turned off).
    assert!(backend.contains("tool: cargo-audit@0.22.2"), "{backend}");
    for dir in ["src-tauri", "shell-ext-win"] {
        assert!(
            backend.contains(&format!(
                "working-directory: {dir}\n        run: cargo audit\n"
            )),
            "backend does not audit {dir}"
        );
    }
    assert!(!ci.contains("cargo audit --ignore") && !ci.contains("cargo audit -i"));
    let audit = read("src-tauri/.cargo/audit.toml");
    let ignored: Vec<&str> = audit
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("\"RUSTSEC-"))
        .collect();
    assert!(!ignored.is_empty(), "{audit}");
    for id in ignored {
        // Every ignored advisory carries its reason on the lines above it.
        let at = audit.find(id).unwrap();
        let above = audit[..at].trim_end().lines().last().unwrap_or("").trim();
        assert!(above.starts_with('#'), "{id} is ignored without a reason");
    }

    for gone in ["llvm-cov", "--ignored", "tauri build", "generate-notices"] {
        assert!(!ci.contains(gone), "ci.yml still runs {gone}");
    }
}

/// THIRD-PARTY-NOTICES.md ships in every bundle, so a release cannot be built
/// from a copy the lockfiles have moved past: the guard regenerates it.
#[test]
fn release_guard_checks_third_party_notices() {
    let guard = job(&release_yml(), "guard");
    assert!(guard.contains("scripts/licenses/generate-notices.sh --check"));
    assert!(guard.contains("tool: cargo-about@0.9.2"));
    let build = job(&release_yml(), "build");
    assert!(build.contains("needs: [guard]"));
}

fn python() -> Option<&'static str> {
    ["python3", "python"].into_iter().find(|p| {
        Command::new(p)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

fn tools(args: &[&str]) -> Option<std::process::Output> {
    let py = python()?;
    Some(
        Command::new(py)
            .arg("-I")
            .arg(repo().join("scripts/release/release_tools.py"))
            .args(args)
            .output()
            .expect("run release_tools.py"),
    )
}

/// FR-086: the beta pointer's comparison is SemVer precedence.
#[test]
fn fr_086_semver_precedence_of_the_release_tools() {
    let gt = |a: &str, b: &str| tools(&["semver-gt", a, b]).map(|o| o.status.code() == Some(0));
    let Some(_) = gt("1.0.0", "0.9.0") else {
        eprintln!("no python on this machine; release_tools.py not exercised");
        return;
    };
    for (a, b, want) in [
        ("0.2.0", "0.2.0-beta.3", true),
        ("0.2.0-beta.3", "0.2.0", false),
        ("0.2.0-beta.10", "0.2.0-beta.9", true),
        ("0.2.0-beta.2", "0.2.0-alpha.9", true),
        ("0.2.0-rc.1", "0.2.0-beta.9", true),
        ("0.1.5", "0.2.0-beta.3", false),
        ("v0.3.0", "0.2.9", true),
        ("0.2.0", "0.2.0", false),
        ("0.2.0+build.2", "0.2.0", false),
        ("0.2.0-beta", "0.2.0-beta.1", false),
        ("0.2.0-1", "0.2.0-beta", false),
    ] {
        assert_eq!(gt(a, b), Some(want), "{a} > {b}");
    }
    let bad = tools(&["semver-gt", "nightly", "1.0.0"]).unwrap();
    assert_eq!(bad.status.code(), Some(2));
}

/// NFR-070: staging copies one build's files under the release names and
/// leaves the throwaway signatures behind; a build that made two files of one
/// kind, or no update artifact, is refused.
#[test]
fn nfr_070_stage_bundle_names_the_files_and_drops_throwaway_signatures() {
    if python().is_none() {
        eprintln!("no python on this machine; release_tools.py not exercised");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("bundle");
    let put = |rel: &str| {
        let p = bundle.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, rel).unwrap();
    };
    put("dmg/Pluryprint_0.2.0_aarch64.dmg");
    put("macos/Pluryprint.app.tar.gz");
    put("macos/Pluryprint.app.tar.gz.sig");
    std::fs::create_dir_all(bundle.join("macos/Pluryprint.app/Contents")).unwrap();
    let out = dir.path().join("dist");
    let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
    let stage = |label: &str, out: &std::path::Path| {
        tools(&[
            "stage-bundle",
            "--bundle-dir",
            &s(&bundle),
            "--label",
            label,
            "--version",
            "0.2.0",
            "--out",
            &s(out),
        ])
        .unwrap()
    };
    let ok = stage("apple_silicon", &out);
    assert!(ok.status.success(), "{ok:?}");
    let mut names: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "Pluryprint_0.2.0_apple_silicon.app.tar.gz",
            "Pluryprint_0.2.0_apple_silicon.dmg"
        ]
    );
    assert_eq!(
        std::fs::read_to_string(out.join("Pluryprint_0.2.0_apple_silicon.app.tar.gz")).unwrap(),
        "macos/Pluryprint.app.tar.gz"
    );

    // The macOS bundle has no NSIS installer: no update artifact for Windows.
    let wrong = stage("windows_x64", &dir.path().join("w"));
    assert_eq!(wrong.status.code(), Some(1), "{wrong:?}");
    // An unknown label.
    assert_eq!(stage("linux", &dir.path().join("l")).status.code(), Some(1));
    // Two installers in one build would get one name.
    put("nsis/Pluryprint_0.2.0_x64-setup.exe");
    put("nsis/Pluryprint_0.1.0_x64-setup.exe");
    let twice = stage("windows_x64", &dir.path().join("t"));
    assert_eq!(twice.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&twice.stderr).contains("-setup.exe"));

    // A folder that already holds files (Vite's dist/) is refused.
    let shared = dir.path().join("shared");
    std::fs::create_dir_all(shared.join("assets")).unwrap();
    std::fs::write(shared.join("index.html"), "<!doctype html>").unwrap();
    let mixed = stage("apple_silicon", &shared);
    assert_eq!(mixed.status.code(), Some(1), "{mixed:?}");
    assert!(String::from_utf8_lossy(&mixed.stderr).contains("not empty"));
}

/// The build job stages into its own folder, never Vite's `dist/`.
#[test]
fn nfr_070_release_files_are_not_staged_in_vites_dist() {
    let release = read(".github/workflows/release.yml");
    assert!(release.contains("--out release-staging"));
    assert!(release.contains("path: release-staging/"));
    assert!(!release.contains("--out dist"));
    assert!(!release.contains("path: dist/"));
}

/// FR-085/NFR-022: latest.json is written from the signed files and their
/// uploaded assets: every platform points at an asset of this release in the
/// API form and carries the sign job's signature; a platform without a
/// signature or an upload, or one missing, is refused; the release body
/// becomes the update notes. check-manifest and extract-signatures read it.
#[test]
fn fr_085_make_manifest_uses_signed_uploads_and_refuses_a_lost_platform() {
    if python().is_none() {
        eprintln!("no python on this machine; release_tools.py not exercised");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().join("assets");
    std::fs::create_dir_all(&d).unwrap();
    let write = |name: &str, text: &str| {
        let p = dir.path().join(name);
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    };
    let asset = |name: &str, text: &str| std::fs::write(d.join(name), text).unwrap();
    asset("Pluryprint_0.2.0_apple_silicon.app.tar.gz", "a");
    asset(
        "Pluryprint_0.2.0_apple_silicon.app.tar.gz.sig",
        "c2lnMQ==\n",
    );
    asset("Pluryprint_0.2.0_apple_silicon.dmg", "d");
    asset("Pluryprint_0.2.0_windows_x64.exe", "w");
    asset("Pluryprint_0.2.0_windows_x64.exe.sig", "c2lnMw==");
    let assets = write(
        "assets.json",
        r#"[{"name":"Pluryprint_0.2.0_apple_silicon.app.tar.gz","id":11},
            {"name":"Pluryprint_0.2.0_apple_silicon.dmg","id":12},
            {"name":"Pluryprint_0.2.0_windows_x64.exe","id":13}]"#,
    );
    let body = write("body.md", "  What changed  \n");
    let out = dir.path().join("latest.json");
    let out_s = out.to_string_lossy().into_owned();
    let d_s = d.to_string_lossy().into_owned();
    let base = [
        "make-manifest",
        "--dir",
        &d_s,
        "--version",
        "v0.2.0",
        "--assets",
        &assets,
        "--repo",
        "MAECLY/pluryprint",
        "--body-file",
        &body,
        "--pub-date",
        "2026-10-10T00:00:00Z",
        "--out",
        &out_s,
    ];
    let ok = tools(
        &[
            &base[..],
            &["--require", "darwin-aarch64", "--require", "windows-x86_64"],
        ]
        .concat(),
    )
    .unwrap();
    assert!(ok.status.success(), "{ok:?}");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let api = "https://api.github.com/repos/MAECLY/pluryprint/releases/assets/";
    assert_eq!(v["version"], "0.2.0");
    assert_eq!(v["notes"], "What changed");
    assert_eq!(v["pub_date"], "2026-10-10T00:00:00Z");
    assert_eq!(v["platforms"]["darwin-aarch64"]["url"], format!("{api}11"));
    assert_eq!(v["platforms"]["darwin-aarch64"]["signature"], "c2lnMQ==");
    assert_eq!(v["platforms"]["windows-x86_64"]["url"], format!("{api}13"));
    assert_eq!(
        v["platforms"].as_object().unwrap().len(),
        2,
        "the dmg is not an update artifact: {v}"
    );

    // windows-aarch64 is missing: the build for it failed.
    let lost = tools(&[&base[..], &["--require", "windows-aarch64"]].concat()).unwrap();
    assert_eq!(lost.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&lost.stderr).contains("windows-aarch64"));

    // A file the sign job did not sign.
    asset("Pluryprint_0.2.0_apple_intel.app.tar.gz", "i");
    let unsigned = tools(&base).unwrap();
    assert_eq!(unsigned.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&unsigned.stderr).contains("apple_intel.app.tar.gz.sig"));

    // Signed, but never uploaded to this release.
    asset("Pluryprint_0.2.0_apple_intel.app.tar.gz.sig", "c2lnMg==");
    let not_uploaded = tools(&base).unwrap();
    assert_eq!(not_uploaded.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&not_uploaded.stderr).contains("not an asset of this release"));
    std::fs::remove_file(d.join("Pluryprint_0.2.0_apple_intel.app.tar.gz")).unwrap();
    std::fs::remove_file(d.join("Pluryprint_0.2.0_apple_intel.app.tar.gz.sig")).unwrap();

    // check-manifest: the version must be the tag's.
    let wrong = tools(&["check-manifest", "--manifest", &out_s, "--version", "0.3.0"]).unwrap();
    assert_eq!(wrong.status.code(), Some(1));
    let right = tools(&[
        "check-manifest",
        "--manifest",
        &out_s,
        "--version",
        "0.2.0",
        "--require",
        "darwin-aarch64",
    ])
    .unwrap();
    assert!(right.status.success(), "{right:?}");

    // NFR-022: the signature of each platform is decoded next to, not into,
    // the assets, and paired with the file its API URL names by id.
    let sigs = dir.path().join("sigcheck");
    let sigs_s = sigs.to_string_lossy().into_owned();
    let extracted = tools(&[
        "extract-signatures",
        "--manifest",
        &out_s,
        "--assets",
        &assets,
        "--out",
        &sigs_s,
        "--require",
        "darwin-aarch64",
        "--require",
        "windows-x86_64",
    ])
    .unwrap();
    assert!(extracted.status.success(), "{extracted:?}");
    assert_eq!(
        String::from_utf8_lossy(&extracted.stdout),
        "darwin-aarch64 Pluryprint_0.2.0_apple_silicon.app.tar.gz\n\
         windows-x86_64 Pluryprint_0.2.0_windows_x64.exe\n"
    );
    // The manifest carries the signature in base64; minisign reads the
    // decoded bytes.
    assert_eq!(
        std::fs::read(sigs.join("darwin-aarch64.minisig")).unwrap(),
        b"sig1"
    );
    let missing = tools(&[
        "extract-signatures",
        "--manifest",
        &out_s,
        "--assets",
        &assets,
        "--out",
        &sigs_s,
        "--require",
        "windows-aarch64",
    ])
    .unwrap();
    assert_eq!(missing.status.code(), Some(1));
}

/// `scripts/bump-version.sh X.Y.Z[-pre]` writes the version into Cargo.toml,
/// Cargo.lock, package.json and the Explorer extension together, and refuses
/// a non-SemVer one.
#[cfg(unix)]
#[test]
fn release_bump_version_script_keeps_the_sources_in_step() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src-tauri")).unwrap();
    std::fs::create_dir_all(root.join("shell-ext-win/res")).unwrap();
    for f in [
        "src-tauri/Cargo.toml",
        "src-tauri/Cargo.lock",
        "package.json",
        "shell-ext-win/Cargo.toml",
        "shell-ext-win/Cargo.lock",
        "shell-ext-win/res/shellext.rc",
    ] {
        std::fs::copy(repo().join(f), root.join(f)).unwrap();
    }
    let run = |version: &str| {
        Command::new("bash")
            .arg(repo().join("scripts/bump-version.sh"))
            .arg("--root")
            .arg(root)
            .arg("--no-cargo")
            .arg(version)
            .output()
            .expect("run bump-version.sh")
    };
    let out = run("9.8.7-beta.1");
    assert!(out.status.success(), "{out:?}");

    let cargo = std::fs::read_to_string(root.join("src-tauri/Cargo.toml")).unwrap();
    assert!(cargo.contains("\nversion = \"9.8.7-beta.1\"\n"), "{cargo}");
    // Only the package's own version moved, not a dependency's.
    assert_eq!(cargo.matches("9.8.7-beta.1").count(), 1);
    let lock = std::fs::read_to_string(root.join("src-tauri/Cargo.lock")).unwrap();
    assert!(lock.contains("name = \"pluryprint\"\nversion = \"9.8.7-beta.1\""));
    let pkg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("package.json")).unwrap()).unwrap();
    assert_eq!(pkg["version"], "9.8.7-beta.1");

    // The Explorer extension moves with the app (#63).
    let ext = std::fs::read_to_string(root.join("shell-ext-win/Cargo.toml")).unwrap();
    assert!(ext.contains("\nversion = \"9.8.7-beta.1\"\n"), "{ext}");
    assert_eq!(ext.matches("9.8.7-beta.1").count(), 1);
    let ext_lock = std::fs::read_to_string(root.join("shell-ext-win/Cargo.lock")).unwrap();
    assert!(ext_lock.contains("name = \"pluryprint-shellext\"\nversion = \"9.8.7-beta.1\""));
    let rc = std::fs::read_to_string(root.join("shell-ext-win/res/shellext.rc")).unwrap();
    assert_eq!(rc.matches(" 9,8,7,0\n").count(), 2, "{rc}");
    assert_eq!(rc.matches("\"9.8.7.0\"").count(), 2, "{rc}");

    for bad in ["1.2", "v1.2.3", "1.2.3-", "01.2.3"] {
        let out = run(bad);
        assert!(!out.status.success(), "{bad} was accepted");
    }
    assert!(root.join("package.json").exists());
}

/// The landing's `llms.txt` (in MAECLY/pluryprint-landing) tells agents that
/// the dependency install consent flags a Store component that may cost money
/// and never shows a price, quoting this dialog. The landing repository checks
/// its own side; this keeps the dialog from drifting away from it unnoticed.
/// Change both together.
#[test]
fn install_dialog_paid_notice_matches_what_the_landing_quotes() {
    let dialog: serde_json::Value =
        serde_json::from_str(&read("src/i18n/locales/en/printJob.json")).unwrap();
    assert_eq!(
        dialog["install"]["paid"],
        "This component may cost money in the Microsoft Store."
    );
}
