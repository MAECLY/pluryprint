//! FR-124/FR-125 cross-repo E2E, second half: what the app does with the
//! handoff a real PluryFact run left behind.
//!
//! PluryFact's `bun run e2e:pluryprint` drives the extension in Chromium
//! against the real `pluryprint-bridge` (with `PLURYPRINT_BRIDGE_NO_LAUNCH=1`,
//! so the app is never started). The bridge stops after writing the inbox and
//! the handoff file. This test picks up from there, with no window:
//!
//! 1. Rebuilds the exact command line the bridge would have started the app
//!    with (`launch::gui_args`), checks it classifies as a context-menu launch
//!    and parses, and runs it through the app's own gate
//!    (`cli::selection_from_at`: inside the handoff folder, schema version,
//!    size caps, label and overrides, and freshness — 60 s, so run it right
//!    after the browser part, as PluryFact's script does).
//! 2. Checks the selection the Print Job screen would open: source
//!    `pluryfact`, the label, the action, every document inside this job's
//!    inbox folder, in `index` order, with a supported extension.
//! 3. Converts every document the way the print pipeline does before it
//!    spools (`convert_to_pdf`, through the real render worker): a PDF passes
//!    through untouched, an SRI XML becomes a RIDE-like page and the TXT
//!    report a text page. Nothing reaches a printer.
//!
//! Everything is gated: the test is `#[ignore]`d and skips unless
//! `PLURYPRINT_E2E_DATA_DIR` names the bridge's data folder. Optional:
//! `PLURYPRINT_E2E_SAME_HOME=1` additionally requires that the app's own
//! handoff folder (from `$HOME`) *is* that folder — true when the run used
//! `pluryprint-cli bridge install` under a temporary `HOME`, so the app
//! started under that `HOME` would read exactly this file;
//! `PLURYPRINT_E2E_JOB=<jobId>` picks that job's handoff when the run left
//! more than one; `PLURYPRINT_E2E_EXPECT_FILES=<n>` the number of documents;
//! `PLURYPRINT_E2E_OUT=<dir>` keeps the converted PDFs for inspection.
//!
//! ```sh
//! PLURYPRINT_E2E_DATA_DIR=/tmp/…/datos-pluryprint \
//!   cargo test --test bridge_handoff_e2e -- --ignored --nocapture
//! ```
//!
//! Documented in docs/08-development/testing-strategy.md ("Cross-repo E2E with
//! PluryFact").

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use pluryprint_lib::bridge::{self, launch};
use pluryprint_lib::cli::{self, args::Cli, handoff, Intent};
use pluryprint_lib::engine::{
    self,
    convert::{convert_to_pdf, PageSetup},
};

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The PluryFact handoff file to check in `<data>/handoff/`: the one for
/// `PLURYPRINT_E2E_JOB` when set, else the only one there.
fn the_handoff(dir: &Path) -> PathBuf {
    if let Ok(job) = std::env::var("PLURYPRINT_E2E_JOB") {
        let file = dir.join(format!("pluryfact-{job}.json"));
        assert!(
            file.is_file(),
            "no handoff for job {job}: {}",
            file.display()
        );
        return file;
    }
    let files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("no handoff folder at {}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("pluryfact-") && n.ends_with(".json"))
        })
        .collect();
    assert_eq!(
        files.len(),
        1,
        "expected exactly one PluryFact handoff in {}, found {files:?}",
        dir.display()
    );
    files.into_iter().next().unwrap()
}

#[test]
#[ignore = "cross-repo E2E: needs PLURYPRINT_E2E_DATA_DIR from PluryFact's e2e:pluryprint"]
fn fr_125_the_app_accepts_and_converts_a_real_pluryfact_handoff() {
    let Some(data) = env_path("PLURYPRINT_E2E_DATA_DIR") else {
        eprintln!("PLURYPRINT_E2E_DATA_DIR is not set: skipping the cross-repo handoff check");
        return;
    };
    let handoff_dir = bridge::handoff_dir(&data);
    let file = the_handoff(&handoff_dir);
    println!("handoff: {}", file.display());

    if std::env::var_os("PLURYPRINT_E2E_SAME_HOME").is_some_and(|v| v == "1") {
        assert_eq!(
            handoff::handoff_dir().canonicalize().unwrap(),
            handoff_dir.canonicalize().unwrap(),
            "the app under this HOME reads handoffs from another folder"
        );
        println!("ok: the app's own handoff folder under this HOME is the bridge's");
    }

    // 1. The command line the bridge starts the app with, through the app's gate.
    let raw = handoff::read_file(&file).expect("the handoff parses as schema v1");
    let action = raw.action.expect("the bridge always names the action");
    let mut argv = vec!["pluryprint".to_string()];
    argv.extend(launch::gui_args(&file, action, raw.profile_id.as_deref()));
    println!("app argv: {}", argv[1..].join(" "));
    assert_eq!(cli::classify(&argv[1..]), Intent::ContextMenu);
    let parsed = Cli::parse_context_menu(&argv).expect("the app parses the bridge's argv");
    let selection = cli::selection_from_at(&handoff_dir, &parsed).unwrap_or_else(|e| {
        panic!(
            "the app refused the handoff: {} (a stale one means the check ran over {:?} \
             after the bridge wrote it)",
            e.error,
            handoff::MAX_HANDOFF_AGE
        )
    });
    println!(
        "ok: accepted by the context-menu gate (source={:?}, label={:?}, action={}, files={})",
        selection.source,
        selection.label,
        selection.action,
        selection.paths.len()
    );

    // 2. The selection the Print Job screen opens with.
    assert_eq!(selection.source.as_deref(), Some("pluryfact"));
    let label = selection
        .label
        .as_deref()
        .expect("PluryFact labels the batch");
    assert!(!label.is_empty() && label.chars().count() <= 120, "{label}");
    assert!(
        !label.contains("0999999999001"),
        "the label must not carry the RUC: {label}"
    );
    if let Some(n) = env_path("PLURYPRINT_E2E_EXPECT_FILES") {
        let n: usize = n.to_string_lossy().parse().expect("a number");
        assert_eq!(selection.paths.len(), n);
    }
    let inbox = bridge::inbox_dir(&data).canonicalize().unwrap();
    let job_dir = Path::new(&selection.paths[0])
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap();
    assert_eq!(job_dir.parent(), Some(inbox.as_path()));
    let names: Vec<String> = selection
        .paths
        .iter()
        .map(|p| {
            let p = Path::new(p);
            assert!(p.is_absolute(), "{}", p.display());
            assert_eq!(
                p.parent().unwrap().canonicalize().unwrap(),
                job_dir,
                "every document is in this job's inbox folder"
            );
            assert!(
                engine::is_supported(&engine::extension_of(p)),
                "{} is not printable",
                p.display()
            );
            p.file_name().unwrap().to_string_lossy().into_owned()
        })
        .collect();
    for (i, name) in names.iter().enumerate() {
        assert!(
            name.starts_with(&format!("{i:04}-")),
            "print order is index order: {name}"
        );
    }
    // The Print Job screen's own check of the list (`inspect_files`): every
    // document printable, none dropped as an unsupported type.
    let infos = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(pluryprint_lib::ipc::jobs::inspect_files(
            selection.paths.clone(),
        ))
        .expect("inspect_files");
    for info in &infos {
        assert!(
            info.supported && !info.is_dir,
            "Print Job would reject {}: {:?}",
            info.name,
            info.reason
        );
    }
    println!(
        "ok: Print Job lists {} printable files ({})",
        infos.len(),
        infos
            .iter()
            .map(|i| i.ext.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // 3. What the print pipeline does with each one before spooling.
    std::env::set_var("PLURYPRINT_WORKER_EXE", env!("CARGO_BIN_EXE_pluryprint"));
    let scratch = tempfile::tempdir().unwrap();
    let out = env_path("PLURYPRINT_E2E_OUT");
    if let Some(out) = &out {
        std::fs::create_dir_all(out).unwrap();
    }
    let a4 = PageSetup {
        css_size: Some("210mm 297mm".into()),
    };
    for (i, path) in selection.paths.iter().enumerate() {
        let path = Path::new(path);
        let ext = engine::extension_of(path);
        let bytes = std::fs::read(path).unwrap();
        let converted = convert_to_pdf(
            path,
            scratch.path(),
            &format!("e2e-{i}"),
            &AtomicBool::new(false),
            &|_| {},
            Some(Duration::from_secs(120)),
            &a4,
            None,
        )
        .unwrap_or_else(|e| panic!("{} does not convert: {e:?}", names[i]));
        let outcome = match &converted {
            None => {
                // The engine checked the header and hands the original to
                // the spooler untouched (PluryFact's fixtures are tiny
                // stand-ins, so their pages are not counted here).
                assert_eq!(ext, "pdf", "only a PDF prints as is");
                assert!(bytes.starts_with(b"%PDF-"));
                "printed as is".to_string()
            }
            Some(pdf) => {
                assert_ne!(ext, "pdf");
                let doc = lopdf::Document::load(pdf).expect("the converter wrote a PDF");
                let pages = doc.get_pages().len();
                assert!(pages >= 1, "{} converted to an empty PDF", names[i]);
                if let Some(out) = &out {
                    let name = format!("{}.pdf", names[i]);
                    std::fs::copy(pdf, out.join(name)).unwrap();
                }
                format!("converted to a {pages}-page PDF")
            }
        };
        if ext == "xml" {
            // The page the converter laid out is the RIDE-like one, not the tree.
            let html =
                engine::convert::xml::to_html(&bytes, &names[i], engine::convert::xml::Lang::Es)
                    .expect("the XML converter reads what the bridge accepted");
            assert!(
                html.contains("Clave de acceso"),
                "an SRI receipt prints as a RIDE-like page, not as a tree"
            );
            assert!(html.contains("Representación impresa generada por Pluryprint"));
        }
        println!("ok: {} → {outcome}", names[i]);
    }
}
