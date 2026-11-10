//! The Explorer context-menu label cache (`<data_dir>/shell-menu.json`).
//!
//! The Windows shell extension (`shell-ext-win`) draws its menu on Explorer's
//! UI thread and cannot ask the app anything while it does, so it reads a small
//! pre-localized cache instead (`shell-ext-win/src/strings.rs`). This module is
//! the writer: it renders the labels from the same i18n catalog the UI uses
//! (`src/i18n/locales/<lang>/common.json`, section `shellMenu`) and lists up to
//! [`MAX_MENU_PROFILES`] profiles — built-ins first, then customs by name, the
//! order `list_profiles` returns.
//!
//! [`refresh`] is called at startup and after every change that alters the
//! menu: profiles saved, deleted, imported or re-defaulted, and the UI language
//! changed. It is best effort — a failure is logged, never surfaced: the DLL
//! falls back to built-in English labels, which is still a working menu.
//!
//! The file is only written on Windows (nothing else reads it); building and
//! writing it are portable and tested everywhere. The shape is pinned against
//! the reader by `shell-ext-win/tests/fixtures/shell-menu.es.json`, which both
//! crates' tests read.
//!
//! Linux has no cache to write: its file-manager menus are static files in
//! `src-tauri/linux/` (the app's desktop entry, a Dolphin service menu and
//! three Nemo actions), each label written out in English and Spanish. The
//! `linux_files` tests keep them honest against the same catalogs, the
//! engine's format list and the command line they run.

use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::profiles::Profile;

/// File name inside the app's local data directory. Must match
/// `shell_ext_win::strings::CACHE_FILE`.
pub const CACHE_FILE: &str = "shell-menu.json";

/// Must match `shell_ext_win::strings::CACHE_SCHEMA_VERSION`.
pub const CACHE_SCHEMA_VERSION: u32 = 1;

/// Must match `shell_ext_win::strings::MAX_MENU_PROFILES`; more would be
/// dropped by the reader anyway.
pub const MAX_MENU_PROFILES: usize = 8;

/// The catalogs, compiled in so the writer needs no frontend at run time.
const EN_COMMON: &str = include_str!("../../src/i18n/locales/en/common.json");
const ES_COMMON: &str = include_str!("../../src/i18n/locales/es/common.json");

/// The cache document. Field names are the reader's contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShellMenuCache {
    /// Format version; must equal [`CACHE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Language subtag of the labels (`en`, `es`). Diagnostics only.
    pub lang: String,
    /// The localized menu labels.
    pub labels: Labels,
    /// Profiles offered under "Print with …", in menu order (at most
    /// [`MAX_MENU_PROFILES`]).
    pub profiles: Vec<MenuProfile>,
}

/// Labels in the reader's placeholder syntax (`{count}`, `{name}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Labels {
    /// The top-level menu entry ("Pluryprint").
    pub root: String,
    /// "Print" entry for a single selected file.
    pub print_one: String,
    /// "Print" entry for several files; `{count}` is the number selected.
    pub print_many: String,
    /// One entry per profile; `{name}` is the profile name.
    pub print_with_profile: String,
    /// Entry that opens the print dialog with the selection.
    pub configure: String,
    /// Entry that adds the selection to the queue without printing.
    pub queue: String,
    /// Hover text of the top-level entry.
    pub tooltip: String,
}

/// A profile the menu offers by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MenuProfile {
    /// Profile id the extension writes into the handoff's `profile_id`.
    pub id: String,
    /// Display name shown in the menu entry.
    pub name: String,
}

/// The catalog language for an app locale (`es-ES` → `es`); anything without
/// a catalog is English, the same fallback the UI uses.
fn catalog_lang(locale: &str) -> (&'static str, &'static str) {
    let lang = locale
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match lang.as_str() {
        "es" => ("es", ES_COMMON),
        _ => ("en", EN_COMMON),
    }
}

/// One `shellMenu.<key>` string, converted from i18n's `{{param}}` to the
/// reader's `{param}`. Falls back to English for a key missing in the catalog
/// (the parity test makes that impossible for shipped catalogs).
fn label(catalog: &str, key: &str) -> String {
    let lookup = |raw: &str| -> Option<String> {
        let doc: serde_json::Value = serde_json::from_str(raw).ok()?;
        doc.get("shellMenu")?
            .get(key)?
            .as_str()
            .map(|s| s.replace("{{", "{").replace("}}", "}"))
    };
    lookup(catalog)
        .or_else(|| lookup(EN_COMMON))
        .unwrap_or_default()
}

/// Build the cache for `locale` and the profiles in display order.
pub fn build(locale: &str, profiles: &[Profile]) -> ShellMenuCache {
    let (lang, catalog) = catalog_lang(locale);
    ShellMenuCache {
        schema_version: CACHE_SCHEMA_VERSION,
        lang: lang.to_string(),
        labels: Labels {
            // The product name is not translated.
            root: "Pluryprint".to_string(),
            print_one: label(catalog, "print_one"),
            print_many: label(catalog, "print_other"),
            print_with_profile: label(catalog, "printWithProfile"),
            configure: label(catalog, "configure"),
            queue: label(catalog, "queue"),
            tooltip: label(catalog, "tooltip"),
        },
        profiles: profiles
            .iter()
            .take(MAX_MENU_PROFILES)
            .map(|p| MenuProfile {
                id: p.id.clone(),
                name: p.name.clone(),
            })
            .collect(),
    }
}

/// Write `cache` into `dir` atomically (temp file + rename), so Explorer never
/// reads half a file.
///
/// The temp file has a **unique** name. The GUI (startup, profile and settings
/// commands) and the CLI (`profiles …`) refresh the cache from separate
/// processes, and with one fixed `shell-menu.json.tmp` a second writer's
/// truncate-and-write could land in the file the first was renaming, leaving
/// a mixed JSON document the shell extension rejects (English labels, no
/// profiles). Each writer now renames only its own complete file; the last
/// rename wins.
pub fn write_to(dir: &Path, cache: &ShellMenuCache) -> io::Result<PathBuf> {
    use std::io::Write as _;
    std::fs::create_dir_all(dir)?;
    let path = dir.join(CACHE_FILE);
    let bytes = serde_json::to_vec_pretty(cache).map_err(io::Error::other)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(&format!("{CACHE_FILE}."))
        .suffix(".tmp")
        .tempfile_in(dir)?;
    tmp.write_all(&bytes)?;
    tmp.flush()?;
    // `persist` replaces an existing cache file on every platform; on failure
    // the temp file is removed when `tmp` drops.
    tmp.persist(&path).map_err(|e| e.error)?;
    Ok(path)
}

/// Rebuild the cache from the real config and profiles. Windows only; a no-op
/// elsewhere. Never fails: problems are logged.
pub fn refresh() {
    #[cfg(windows)]
    refresh_at(
        &crate::util::paths::data_dir(),
        &crate::settings::Config::config_path(),
        &crate::profiles::profiles_path(),
    );
}

/// [`refresh`] with injected paths: reads the locale from `config_path` and
/// the profiles from `profiles_path`, writes into `data_dir`.
pub fn refresh_at(data_dir: &Path, config_path: &Path, profiles_path: &Path) {
    let locale = crate::settings::Config::load_strict(config_path)
        .map(|c| c.app.locale)
        .unwrap_or_default();
    let profiles = match crate::ipc::profiles::list_profiles_at(profiles_path, config_path) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(event = "shell_menu_cache_failed", error = %e);
            return;
        }
    };
    match write_to(data_dir, &build(&locale, &profiles)) {
        Ok(path) => tracing::debug!(event = "shell_menu_cache_written", path = %path.display()),
        Err(e) => tracing::warn!(event = "shell_menu_cache_failed", error = %e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles(n: usize) -> Vec<Profile> {
        let all = crate::profiles::builtins();
        let base = all.first().expect("there are built-in profiles").clone();
        (0..n)
            .map(|i| Profile {
                id: format!("p{i}"),
                name: format!("Profile {i}"),
                ..base.clone()
            })
            .collect()
    }

    /// FR-001/FR-092: Spanish labels come from the shared catalog, in the
    /// reader's placeholder syntax.
    #[test]
    fn fr_001_spanish_labels_from_the_catalog() {
        let c = build("es-ES", &[]);
        assert_eq!(c.lang, "es");
        assert_eq!(c.labels.print_one, "Imprimir 1 archivo");
        assert_eq!(c.labels.print_many, "Imprimir {count} archivos");
        assert_eq!(c.labels.print_with_profile, "Imprimir con «{name}»");
        assert!(!c.labels.configure.contains("{{"));
    }

    #[test]
    fn fr_001_unknown_locales_fall_back_to_english() {
        for locale in ["en-US", "fr-FR", ""] {
            let c = build(locale, &[]);
            assert_eq!(c.lang, "en", "{locale}");
            assert_eq!(c.labels.print_many, "Print {count} files");
        }
    }

    /// FR-027: the menu lists at most eight profiles, in the order given.
    #[test]
    fn fr_027_profiles_are_capped_and_ordered() {
        let c = build("en-US", &profiles(11));
        assert_eq!(c.profiles.len(), MAX_MENU_PROFILES);
        assert_eq!(c.profiles[0].id, "p0");
        assert_eq!(c.profiles[7].name, "Profile 7");
    }

    #[test]
    fn write_is_atomic_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_to(dir.path(), &build("es-ES", &profiles(2))).unwrap();
        assert_eq!(path, dir.path().join(CACHE_FILE));
        assert!(!dir.path().join(format!("{CACHE_FILE}.tmp")).exists());
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["schema_version"], CACHE_SCHEMA_VERSION);
        assert_eq!(v["profiles"].as_array().unwrap().len(), 2);
    }

    /// The writer's output for a fixed input is byte-for-byte the fixture the
    /// shell extension's own tests parse, so a field renamed on either side
    /// fails one of the two crates.
    #[test]
    fn fr_001_matches_the_fixture_the_shell_extension_reads() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../shell-ext-win/tests/fixtures/shell-menu.es.json");
        let expected = std::fs::read_to_string(&fixture).expect("fixture exists");
        let actual = serde_json::to_string_pretty(&build("es-ES", &profiles(2))).unwrap();
        assert_eq!(actual.trim_end(), expected.trim_end());
    }

    /// The end-to-end path with injected files: locale from config.toml,
    /// profiles from profiles.toml (built-ins only here).
    #[test]
    fn refresh_at_reads_locale_and_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        let mut cfg = crate::settings::Config::default();
        cfg.app.locale = "es-ES".into();
        cfg.save_to(&config).unwrap();
        refresh_at(dir.path(), &config, &dir.path().join("profiles.toml"));
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(CACHE_FILE)).unwrap())
                .unwrap();
        assert_eq!(v["lang"], "es");
        let builtins = crate::profiles::builtins();
        assert_eq!(
            v["profiles"].as_array().unwrap().len(),
            builtins.len().min(MAX_MENU_PROFILES)
        );
        assert_eq!(v["profiles"][0]["id"], builtins[0].id.as_str());
    }

    /// GUI and CLI refresh the cache concurrently: every write must leave a
    /// complete, parseable file (never a mix of two writers), and no temp file.
    #[test]
    fn fr_027_concurrent_writers_never_leave_a_mixed_file() {
        let dir = tempfile::tempdir().unwrap();
        let caches: Vec<ShellMenuCache> = (1..=6)
            .map(|n| build(if n % 2 == 0 { "es" } else { "en" }, &profiles(n)))
            .collect();
        std::thread::scope(|scope| {
            for cache in &caches {
                let dir = dir.path();
                scope.spawn(move || {
                    for _ in 0..40 {
                        // On Windows a replace can lose a race to another
                        // writer's rename; the next attempt wins.
                        let _ = write_to(dir, cache);
                    }
                });
            }
        });
        let text = std::fs::read_to_string(dir.path().join(CACHE_FILE)).unwrap();
        let written: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        let profile_count = written["profiles"].as_array().unwrap().len();
        assert!(
            caches.iter().any(|c| c.profiles.len() == profile_count),
            "the file is one writer's whole document"
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name() != CACHE_FILE)
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}

/// The Linux file-manager integration files (`src-tauri/linux/`), checked
/// against what they promise: every printable format and nothing else, the
/// catalog's labels, and Exec lines the command line actually accepts.
///
/// The files are static because the .deb installs them as they are; these
/// tests are what stops them drifting when a format or a label changes.
#[cfg(test)]
mod linux_files {
    use std::path::{Path, PathBuf};

    use super::{EN_COMMON, ES_COMMON};
    use crate::cli::args::{Cli, ContextAction, InvokeSource};

    const DESKTOP_TEMPLATE: &str = include_str!("../linux/pluryprint.desktop.hbs");
    const KIO_SERVICE_MENU: &str = include_str!("../linux/kio-servicemenu.desktop");
    const NEMO_ACTIONS: &[(&str, &str, ContextAction)] = &[
        (
            "pluryprint-print-now.nemo_action",
            include_str!("../linux/nemo/pluryprint-print-now.nemo_action"),
            ContextAction::PrintDefault,
        ),
        (
            "pluryprint-configure.nemo_action",
            include_str!("../linux/nemo/pluryprint-configure.nemo_action"),
            ContextAction::Configure,
        ),
        (
            "pluryprint-queue.nemo_action",
            include_str!("../linux/nemo/pluryprint-queue.nemo_action"),
            ContextAction::Queue,
        ),
    ];

    /// Each printable extension and the shared-mime-info types a file manager
    /// reports for it, aliases included (image/heic is image/heif's alias,
    /// text/x-markdown text/markdown's) so an older or newer MIME database
    /// still matches. Every type here exists in Ubuntu 22.04's
    /// shared-mime-info 2.1.
    const EXT_MIME: &[(&str, &[&str])] = &[
        ("pdf", &["application/pdf"]),
        ("jpg", &["image/jpeg"]),
        ("jpeg", &["image/jpeg"]),
        ("png", &["image/png"]),
        ("tif", &["image/tiff"]),
        ("tiff", &["image/tiff"]),
        ("bmp", &["image/bmp"]),
        ("webp", &["image/webp"]),
        ("heic", &["image/heif", "image/heic"]),
        ("heif", &["image/heif", "image/heic"]),
        ("svg", &["image/svg+xml"]),
        ("svgz", &["image/svg+xml-compressed"]),
        (
            "docx",
            &["application/vnd.openxmlformats-officedocument.wordprocessingml.document"],
        ),
        ("doc", &["application/msword"]),
        (
            "xlsx",
            &["application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"],
        ),
        ("xls", &["application/vnd.ms-excel"]),
        (
            "pptx",
            &["application/vnd.openxmlformats-officedocument.presentationml.presentation"],
        ),
        ("ppt", &["application/vnd.ms-powerpoint"]),
        ("odt", &["application/vnd.oasis.opendocument.text"]),
        ("ods", &["application/vnd.oasis.opendocument.spreadsheet"]),
        ("odp", &["application/vnd.oasis.opendocument.presentation"]),
        ("rtf", &["application/rtf", "text/rtf"]),
        ("txt", &["text/plain"]),
        ("html", &["text/html"]),
        ("htm", &["text/html"]),
        ("md", &["text/markdown", "text/x-markdown"]),
        ("markdown", &["text/markdown", "text/x-markdown"]),
    ];

    /// One `[group]` of a key file and its `key=value` lines, in file order.
    type Group<'a> = (&'a str, Vec<(&'a str, &'a str)>);

    /// A deliberately small key-file reader: groups, `key=value` lines,
    /// comments and blank lines, plus the template's Handlebars block lines.
    /// Anything else, and a key repeated within a group, fails the test.
    fn groups(text: &str) -> Vec<Group<'_>> {
        let mut out: Vec<Group<'_>> = Vec::new();
        for line in text.lines() {
            let line = line.trim_end();
            if line.is_empty()
                || line.starts_with('#')
                || line.starts_with("{{#")
                || line.starts_with("{{/")
            {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                out.push((name, Vec::new()));
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .unwrap_or_else(|| panic!("not a key=value line: {line:?}"));
            let (_, entries) = out.last_mut().expect("a key before any group");
            assert!(
                entries.iter().all(|(k, _)| *k != key),
                "{key} appears twice in one group"
            );
            entries.push((key, value));
        }
        out
    }

    fn get<'a>(group: &Group<'a>, key: &str) -> &'a str {
        group
            .1
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
            .unwrap_or_else(|| panic!("[{}] has no {key}", group.0))
    }

    fn list(value: &str) -> Vec<&str> {
        assert!(value.ends_with(';'), "a list ends with ';': {value}");
        value.trim_end_matches(';').split(';').collect()
    }

    /// The MIME types every printable format needs, deduplicated in
    /// [`crate::engine::SUPPORTED_EXTS`] order.
    fn printable_mime_types() -> Vec<&'static str> {
        let mut out: Vec<&str> = Vec::new();
        for ext in crate::engine::SUPPORTED_EXTS {
            let (_, mimes) = EXT_MIME
                .iter()
                .find(|(e, _)| e == ext)
                .unwrap_or_else(|| panic!("{ext} is printable but has no MIME type here"));
            for mime in *mimes {
                if !out.contains(mime) {
                    out.push(mime);
                }
            }
        }
        out
    }

    /// `%F` expanded the way a file manager expands it: one argument per
    /// selected path.
    fn expand(exec: &str, files: &[PathBuf]) -> Vec<String> {
        exec.split_whitespace()
            .flat_map(|arg| match arg {
                "%F" => files
                    .iter()
                    .map(|f| f.to_string_lossy().into_owned())
                    .collect(),
                "{{exec}}" => vec!["pluryprint".to_string()],
                other => vec![other.to_string()],
            })
            .collect()
    }

    /// The catalog's `shellMenu.<key>` in `catalog`.
    fn catalog_label(catalog: &str, key: &str) -> String {
        let v: serde_json::Value = serde_json::from_str(catalog).unwrap();
        v["shellMenu"][key].as_str().unwrap().to_string()
    }

    fn two_files(dir: &Path) -> Vec<PathBuf> {
        let files = vec![dir.join("a report.pdf"), dir.join("photo.heic")];
        for f in &files {
            std::fs::write(f, b"x").unwrap();
        }
        files
    }

    #[test]
    fn fr_004_every_printable_format_has_a_mime_type_and_no_more() {
        for (ext, _) in EXT_MIME {
            assert!(
                crate::engine::SUPPORTED_EXTS.contains(ext),
                "{ext} is not printable, so no menu may offer it"
            );
        }
        // Panics on a printable extension the table above does not know.
        assert!(!printable_mime_types().is_empty());
    }

    /// "Open with" lists Pluryprint for exactly the printable formats, and
    /// never for folders.
    #[test]
    fn fr_004_the_desktop_entry_opens_every_printable_format() {
        let groups = groups(DESKTOP_TEMPLATE);
        assert_eq!(groups.len(), 1, "one [Desktop Entry], no actions");
        let entry = &groups[0];
        assert_eq!(entry.0, "Desktop Entry");
        assert_eq!(get(entry, "Type"), "Application");
        assert_eq!(list(get(entry, "MimeType")), printable_mime_types());
        assert_eq!(get(entry, "Exec"), "{{exec}} %F");
        // The dock matches the window to this entry through its WM class,
        // which GTK takes from the binary's name.
        assert_eq!(get(entry, "StartupWMClass"), "{{exec}}");
    }

    /// `pluryprint <files>` is an "open with" launch: the GUI opens on the
    /// Print Job screen with the files loaded (FR-008).
    #[test]
    fn fr_008_the_desktop_entry_exec_is_an_open_with_launch() {
        let dir = tempfile::tempdir().unwrap();
        let files = two_files(dir.path());
        let entry = &groups(DESKTOP_TEMPLATE)[0];
        let argv = expand(get(entry, "Exec"), &files);
        assert_eq!(
            crate::cli::classify(&argv[1..]),
            crate::cli::Intent::Gui,
            "{argv:?}"
        );
        let selection = crate::cli::open_with_selection(&argv[1..]).expect("files are loaded");
        assert_eq!(selection.paths.len(), 2);
        assert_eq!(selection.action, ContextAction::Configure);
    }

    /// The Dolphin submenu: the three Explorer actions, on every printable
    /// format and on folders, with the catalog's labels.
    #[test]
    fn fr_004_the_dolphin_service_menu_mirrors_the_explorer_menu() {
        let groups = groups(KIO_SERVICE_MENU);
        let entry = &groups[0];
        assert_eq!(entry.0, "Desktop Entry");
        assert_eq!(get(entry, "Type"), "Service");
        assert_eq!(get(entry, "X-KDE-ServiceTypes"), "KonqPopupMenu/Plugin");
        assert_eq!(get(entry, "X-KDE-Submenu"), "Pluryprint");
        let mut expected = printable_mime_types();
        expected.push("inode/directory");
        assert_eq!(list(get(entry, "MimeType")), expected);
        assert_eq!(
            list(get(entry, "Actions")),
            ["printNow", "configure", "queue"]
        );
        assert_eq!(groups.len(), 4, "the entry and its three actions");

        for (id, label) in [("configure", "configure"), ("queue", "queue")] {
            let action = groups
                .iter()
                .find(|g| g.0 == format!("Desktop Action {id}"))
                .unwrap();
            assert_eq!(get(action, "Name"), catalog_label(EN_COMMON, label));
            assert_eq!(get(action, "Name[es]"), catalog_label(ES_COMMON, label));
        }
    }

    /// Every Nemo action offers itself on the printable extensions and on
    /// folders, only when something is selected, and only while the
    /// `pluryprint` command exists.
    #[test]
    fn fr_004_the_nemo_actions_cover_every_printable_format() {
        let mut expected = vec!["dir".to_string()];
        expected.extend(
            crate::engine::SUPPORTED_EXTS
                .iter()
                .map(|e| format!(".{e}")),
        );
        for (name, text, _) in NEMO_ACTIONS {
            let groups = groups(text);
            assert_eq!(groups.len(), 1, "{name}");
            let action = &groups[0];
            assert_eq!(action.0, "Nemo Action", "{name}");
            assert_eq!(list(get(action, "Extensions")), expected, "{name}");
            assert_eq!(get(action, "Selection"), "notnone", "{name}");
            assert_eq!(get(action, "Dependencies"), "pluryprint;", "{name}");
            assert!(!get(action, "Name[es]").is_empty(), "{name}");
        }
    }

    /// Every menu line parses as the context-menu invocation it means, with
    /// the selected files after `--`, and (on Linux) resolves to a selection
    /// carrying the right action.
    #[test]
    fn fr_004_every_menu_exec_line_is_a_context_menu_invocation() {
        let dir = tempfile::tempdir().unwrap();
        let files = two_files(dir.path());

        let kio = groups(KIO_SERVICE_MENU);
        let mut lines: Vec<(String, &str, ContextAction)> = [
            ("printNow", ContextAction::PrintDefault),
            ("configure", ContextAction::Configure),
            ("queue", ContextAction::Queue),
        ]
        .into_iter()
        .map(|(id, action)| {
            let group = kio
                .iter()
                .find(|g| g.0 == format!("Desktop Action {id}"))
                .unwrap();
            (format!("dolphin {id}"), get(group, "Exec"), action)
        })
        .collect();
        for (name, text, action) in NEMO_ACTIONS {
            lines.push((name.to_string(), get(&groups(text)[0], "Exec"), *action));
        }

        for (name, exec, action) in lines {
            let argv = expand(exec, &files);
            assert_eq!(argv[0], "pluryprint", "{name}");
            assert_eq!(
                crate::cli::classify(&argv[1..]),
                crate::cli::Intent::ContextMenu,
                "{name}"
            );
            let cli = Cli::parse_context_menu(&argv).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(cli.invoke, Some(InvokeSource::ContextMenu), "{name}");
            assert_eq!(cli.action, action, "{name}");
            assert_eq!(cli.paths, files, "{name}");
            assert!(cli.files_from.is_none() && cli.command.is_none(), "{name}");

            let resolved = crate::cli::selection_from_at_cwd(dir.path(), &cli, None);
            if crate::cli::ARGV_SELECTION {
                let selection = resolved.unwrap_or_else(|e| panic!("{name}: {:?}", e.error));
                assert_eq!(selection.action, action, "{name}");
                assert_eq!(selection.paths.len(), 2, "{name}");
            } else {
                assert!(resolved.is_err(), "{name}: argv selections are Linux-only");
            }
        }
    }
}
