//! Which selections the menu entry offers itself for (FR-005).
//!
//! The *authority* on what can be printed is the app: it filters the selection
//! and warns the user about everything it discarded. This module only decides
//! whether showing the entry at all would be pointless — a selection of nothing
//! but `.exe` files should not offer to print.
//!
//! Two constraints make this deliberately shallow:
//!
//! * `GetState` runs on Explorer's UI thread and may be called with
//!   `fOkToBeSlow = FALSE`, so it must never touch the disk;
//! * a selection can hold thousands of items, and inspecting all of them would
//!   stall the menu, so only the first [`PROBE_LIMIT`] are looked at.
//!
//! The result is a decision that can be wrong in one harmless direction: a huge
//! selection whose first few items are unsupported still shows the entry, and
//! the app then explains what it dropped. It is never wrong in the direction
//! that matters (hiding the entry for a printable selection).
//!
//! Portable and unit-testable on any host.

/// Extensions the print engine can turn into a printable PDF.
///
/// Mirrors `SUPPORTED_EXTS` in `src-tauri/src/engine/mod.rs`, which is the
/// source of truth. Duplicated rather than shared because the app is a separate
/// crate; a drift here only widens or narrows what the menu offers, never what
/// actually prints.
pub const SUPPORTED_EXTS: &[&str] = &[
    "pdf", "jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp", "heic", "heif", "svg", "svgz",
    "docx", "doc", "xlsx", "xls", "pptx", "ppt", "odt", "ods", "odp", "rtf", "txt", "html", "htm",
    "md", "markdown", "xml",
];

/// How many items of a selection are inspected before giving up and showing the
/// entry anyway.
pub const PROBE_LIMIT: usize = 16;

/// One inspected item, reduced to what the decision needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeItem {
    /// Display or parsing name — only the extension is read off it.
    pub name: String,
    /// Folders are always offered: the app expands them (FR-004).
    pub is_folder: bool,
}

/// Whether `name` ends in an extension the engine can print. Case-insensitive;
/// a name with no extension is not supported.
pub fn is_supported_name(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if ext.is_empty() || ext.contains('\\') || ext.contains('/') {
        return false;
    }
    SUPPORTED_EXTS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(ext))
}

/// Whether the context-menu entry should be shown for this selection.
///
/// `probed` is the first [`PROBE_LIMIT`] items at most; `total` is the real
/// selection size. When the selection is larger than what was probed the answer
/// is always "show it", because the unseen tail may well be printable.
pub fn should_show(probed: &[ProbeItem], total: usize) -> bool {
    if total == 0 {
        return false;
    }
    if total > probed.len() {
        return true;
    }
    probed
        .iter()
        .any(|item| item.is_folder || is_supported_name(&item.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> ProbeItem {
        ProbeItem {
            name: name.into(),
            is_folder: false,
        }
    }

    #[test]
    fn fr_005_recognises_the_engine_formats_case_insensitively() {
        assert!(is_supported_name("invoice.pdf"));
        assert!(is_supported_name("SCAN.TIFF"));
        assert!(is_supported_name("holiday.JpEg"));
        assert!(is_supported_name("report.docx"));
        assert!(is_supported_name("notes.MD"));
        assert!(!is_supported_name("setup.exe"));
        assert!(!is_supported_name("Makefile"));
        assert!(!is_supported_name("archive."));
        // A dot in a directory component is not an extension.
        assert!(!is_supported_name(r"C:\v1.0\readme"));
    }

    /// The list is duplicated across crates; this keeps it from drifting from
    /// the engine's, which is the source of truth (a stale copy hid the entry
    /// for every Office, text, HTML, Markdown and HEIC file).
    #[test]
    fn fr_005_mirrors_the_engine_list() {
        let engine = include_str!("../../src-tauri/src/engine/mod.rs");
        let start = engine
            .find("pub const SUPPORTED_EXTS: &[&str] = &[")
            .expect("engine SUPPORTED_EXTS");
        let body = &engine[start..];
        let body = &body[body.find('[').unwrap() + 1..];
        let body = &body[body.find('[').unwrap() + 1..body.find("];").unwrap()];
        let engine_exts: Vec<&str> = body
            .split(',')
            .map(|s| s.trim().trim_matches('"'))
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(engine_exts, SUPPORTED_EXTS);
    }

    #[test]
    fn fr_005_hides_the_entry_only_when_nothing_is_printable() {
        assert!(should_show(&[file("a.pdf"), file("b.exe")], 2));
        assert!(!should_show(&[file("a.exe"), file("b.dll")], 2));
        assert!(!should_show(&[], 0));
    }

    #[test]
    fn fr_004_folders_always_qualify() {
        let folder = ProbeItem {
            name: "Invoices".into(),
            is_folder: true,
        };
        assert!(should_show(&[folder], 1));
    }

    #[test]
    fn fr_003_a_selection_larger_than_the_probe_is_always_offered() {
        // The first 16 are unprintable but item 5,000 may not be: never hide
        // an entry on the strength of a sample.
        let probed: Vec<ProbeItem> = (0..PROBE_LIMIT)
            .map(|i| file(&format!("f{i}.exe")))
            .collect();
        assert!(should_show(&probed, 5_000));
        assert!(!should_show(&probed, PROBE_LIMIT));
    }
}
