//! The registry of every stable error `reason` (FR-092).
//!
//! A `reason` is the machine sub-code the UI turns into a sentence in the
//! user's language. Two families exist:
//!
//! - [`IPC_REASONS`]: carried by a [`LocalizedError`] (a command's or the CLI's
//!   failure), translated from `common:ipcErrors.reasons.<Code>.<reason>`;
//! - [`JOB_REASONS`]: carried by a persisted [`JobError`] (a job in the
//!   queue), translated from `queue:jobErrors.reasons.<code>.<reason>` — except
//!   `dependency_missing`, whose reason is the dependency id and whose sentence
//!   is the shared remediation `common:deps.<id>.remediation`, so the Print Job
//!   screen, the queue and the toasts all say the same thing.
//!
//! An engine failure ([`crate::engine::Failure`]) registers its reason under
//! the job code of its variant ([`job_code_of`]). When the same failure crosses
//! the IPC boundary instead (the CLI, a command), it keeps that reason inside
//! the matching IPC family ([`ipc_family_of_job_code`]), so it is a valid IPC
//! reason too ([`is_ipc_reason`]).
//!
//! Every entry also declares the interpolation parameters its sentence may use.
//! The tests below load the EN and ES catalogs and fail when a registered
//! reason has no sentence in either language, or when a sentence uses a
//! `{{param}}` the entry does not declare. Debug builds additionally assert, at
//! construction time, that nothing emits an unregistered reason
//! ([`LocalizedError::new`], [`JobError::with_reason`]).
//!
//! Reasons are a public contract (`queue.json`, the CLI's JSON output): add new
//! ones freely, never rename or remove one that has shipped.
//!
//! [`LocalizedError`]: crate::util::error::LocalizedError
//! [`LocalizedError::new`]: crate::util::error::LocalizedError::new
//! [`JobError`]: crate::queue::job::JobError
//! [`JobError::with_reason`]: crate::queue::job::JobError::with_reason

use crate::util::error::IpcErrorCode;

/// One registered reason: the family it belongs to, its stable name, and the
/// parameters its translated sentence may interpolate. A plural sentence uses
/// a numeric `count` param and is stored as `<reason>_one` / `<reason>_other`.
pub type ReasonSpec<F> = (F, &'static str, &'static [&'static str]);

/// Every reason a [`crate::util::error::LocalizedError`] may carry.
pub const IPC_REASONS: &[ReasonSpec<IpcErrorCode>] = &[
    // --- Update check ---
    (IpcErrorCode::UpdateCheckFailed, "rate_limited", &[]),
    (IpcErrorCode::UpdateCheckFailed, "offline", &[]),
    (IpcErrorCode::UpdateCheckFailed, "bad_status", &["status"]),
    (IpcErrorCode::UpdateCheckFailed, "bad_response", &[]),
    (IpcErrorCode::UpdateCheckFailed, "repo_missing", &[]),
    (IpcErrorCode::UpdateCheckFailed, "not_configured", &[]),
    (IpcErrorCode::UpdateCheckFailed, "channel_unavailable", &[]),
    // --- Update install ---
    (
        IpcErrorCode::UpdateSignatureInvalid,
        "signature_rejected",
        &[],
    ),
    (IpcErrorCode::UpdateInstallFailed, "download_failed", &[]),
    (IpcErrorCode::UpdateInstallFailed, "no_platform_update", &[]),
    (
        IpcErrorCode::UpdateInstallFailed,
        "not_an_installed_app",
        &[],
    ),
    (IpcErrorCode::UpdateInstallFailed, "install_failed", &[]),
    (IpcErrorCode::UpdateInstallFailed, "bad_endpoint", &[]),
    (IpcErrorCode::UpdateInstallFailed, "app_not_located", &[]),
    (
        IpcErrorCode::UpdateInstallFailed,
        "wrong_package_format",
        &[],
    ),
    (
        IpcErrorCode::UpdateInstallFailed,
        "appimage_not_writable",
        &[],
    ),
    (IpcErrorCode::UpdateInstallFailed, "auth_cancelled", &[]),
    (
        IpcErrorCode::UpdateInstallFailed,
        "package_install_failed",
        &[],
    ),
    (
        IpcErrorCode::UpdateInstallFailed,
        "previous_app_not_restored",
        &["path"],
    ),
    // --- Internal ---
    (IpcErrorCode::Internal, "open_logs_failed", &[]),
    (IpcErrorCode::Internal, "open_link_failed", &[]),
    (IpcErrorCode::Internal, "config_corrupt", &[]),
    (IpcErrorCode::Internal, "profiles_corrupt", &[]),
    (IpcErrorCode::Internal, "profiles_invalid_entries", &[]),
    (IpcErrorCode::Internal, "path_unresolvable", &["path"]),
    (IpcErrorCode::Internal, "printer_list_unstable", &[]),
    (IpcErrorCode::Internal, "printer_query_failed", &[]),
    (IpcErrorCode::Internal, "discovery_failed", &[]),
    (IpcErrorCode::Internal, "printer_settings_unavailable", &[]),
    (IpcErrorCode::Internal, "update_state_save_failed", &[]),
    // --- Invalid request: links, profiles, import, updates ---
    (IpcErrorCode::InvalidRequest, "url_not_allowed", &[]),
    (IpcErrorCode::InvalidRequest, "unknown_default_profile", &[]),
    (IpcErrorCode::InvalidRequest, "builtin_readonly", &[]),
    (IpcErrorCode::InvalidRequest, "builtin_delete", &[]),
    (IpcErrorCode::InvalidRequest, "delete_default", &[]),
    (IpcErrorCode::InvalidRequest, "id_too_long", &[]),
    (IpcErrorCode::InvalidRequest, "name_empty", &[]),
    (IpcErrorCode::InvalidRequest, "name_too_long", &[]),
    (IpcErrorCode::InvalidRequest, "name_taken", &["name"]),
    (IpcErrorCode::InvalidRequest, "copies_out_of_range", &[]),
    (IpcErrorCode::InvalidRequest, "page_range", &[]),
    (IpcErrorCode::InvalidRequest, "import_too_large", &[]),
    (IpcErrorCode::InvalidRequest, "import_unreadable", &[]),
    (IpcErrorCode::InvalidRequest, "import_malformed", &[]),
    (
        IpcErrorCode::InvalidRequest,
        "import_unsupported_version",
        &["version"],
    ),
    (IpcErrorCode::InvalidRequest, "import_empty", &[]),
    (
        IpcErrorCode::InvalidRequest,
        "import_invalid_profile",
        &["name"],
    ),
    (
        IpcErrorCode::InvalidRequest,
        "add_printer_unsupported_scheme",
        &["scheme"],
    ),
    (
        IpcErrorCode::InvalidRequest,
        "major_update_unconfirmed",
        &["version", "current"],
    ),
    (
        IpcErrorCode::InvalidRequest,
        "major_update_changed",
        &["confirmed", "version"],
    ),
    (IpcErrorCode::InvalidRequest, "reserved_id", &[]),
    // --- Dependency installation (FR-058, D13) ---
    (
        IpcErrorCode::InvalidRequest,
        "install_consent_required",
        &[],
    ),
    (IpcErrorCode::InvalidRequest, "install_not_offered", &[]),
    (
        IpcErrorCode::InvalidRequest,
        "install_tool_missing",
        &["tool"],
    ),
    (IpcErrorCode::InvalidRequest, "install_in_progress", &[]),
    (IpcErrorCode::Internal, "install_start_failed", &["tool"]),
    // --- Invalid request: submission, settings, page ranges, paths ---
    (IpcErrorCode::InvalidRequest, "no_files", &[]),
    (IpcErrorCode::InvalidRequest, "not_a_file", &["name"]),
    (
        IpcErrorCode::InvalidRequest,
        "file_too_large",
        &["name", "limit"],
    ),
    (IpcErrorCode::InvalidRequest, "page_range_empty", &[]),
    (IpcErrorCode::InvalidRequest, "tray_empty", &[]),
    (
        IpcErrorCode::InvalidRequest,
        "page_range_empty_entry",
        &["position"],
    ),
    (
        IpcErrorCode::InvalidRequest,
        "page_range_incomplete",
        &["entry"],
    ),
    (
        IpcErrorCode::InvalidRequest,
        "page_range_backwards",
        &["entry", "start", "end"],
    ),
    (
        IpcErrorCode::InvalidRequest,
        "page_number_invalid",
        &["token"],
    ),
    (IpcErrorCode::InvalidRequest, "page_number_zero", &[]),
    (
        IpcErrorCode::InvalidRequest,
        "page_range_outside",
        &["name", "span", "count"],
    ),
    (
        IpcErrorCode::InvalidRequest,
        "setting_out_of_range",
        &["field", "min", "max"],
    ),
    (
        IpcErrorCode::InvalidRequest,
        "setting_out_of_range_or_off",
        &["field", "min", "max"],
    ),
    (IpcErrorCode::InvalidRequest, "locale_empty", &[]),
    (IpcErrorCode::InvalidRequest, "path_empty", &[]),
    (IpcErrorCode::InvalidRequest, "path_not_absolute", &["path"]),
    (IpcErrorCode::InvalidRequest, "printer_uri_invalid", &[]),
    (IpcErrorCode::InvalidRequest, "printer_option_invalid", &[]),
    // FR-059: `job_supply_password` with nothing typed.
    (IpcErrorCode::InvalidRequest, "password_empty", &[]),
    // FR-127: "Reconnect" in Settings > Browser integration.
    (IpcErrorCode::InvalidRequest, "bridge_translocated", &[]),
    (IpcErrorCode::Internal, "bridge_missing", &[]),
    // --- Unsupported format (drop-time refusals) ---
    (IpcErrorCode::UnsupportedFormat, "not_printable", &["name"]),
    (
        IpcErrorCode::UnsupportedFormat,
        "gif_not_supported",
        &["name"],
    ),
    // --- Dependencies ---
    (IpcErrorCode::DependencyMissing, "ippfind_missing", &[]),
    (IpcErrorCode::DependencyMissing, "cups_missing", &[]),
    // --- Printers ---
    (IpcErrorCode::PrintFailed, "set_default_failed", &[]),
    (IpcErrorCode::PrintFailed, "add_printer_needs_admin", &[]),
    (IpcErrorCode::PrintFailed, "spooler_down", &[]),
    (IpcErrorCode::PrintFailed, "add_printer_no_ippurl", &[]),
    (IpcErrorCode::PrintFailed, "add_printer_failed", &[]),
    (IpcErrorCode::PrintFailed, "cups_unavailable", &[]),
    (IpcErrorCode::PrintFailed, "test_page_failed", &[]),
    // --- Job actions ---
    (IpcErrorCode::InvalidState, "update_in_progress", &[]),
    (IpcErrorCode::InvalidState, "resume_no_printer", &[]),
    (
        IpcErrorCode::InvalidState,
        "resume_printer_missing",
        &["printer"],
    ),
    (
        IpcErrorCode::InvalidState,
        "resume_printer_ambiguous",
        &["printer"],
    ),
    (
        IpcErrorCode::InvalidState,
        "resume_printer_disabled",
        &["printer"],
    ),
    (IpcErrorCode::InvalidState, "job_finished", &[]),
    (IpcErrorCode::InvalidState, "job_printing", &[]),
    (IpcErrorCode::InvalidState, "job_not_paused", &[]),
    (IpcErrorCode::InvalidState, "job_not_failed", &[]),
    (IpcErrorCode::InvalidState, "job_cancelled", &[]),
    // FR-059: a password was supplied for a job that is not asking for one.
    (IpcErrorCode::InvalidState, "job_not_waiting_password", &[]),
];

/// Every reason a persisted [`crate::queue::job::JobError`] may carry, keyed by
/// its job error `code`.
pub const JOB_REASONS: &[ReasonSpec<&'static str>] = &[
    // --- Queue-level (not engine) ---
    ("conversion_timeout", "converter", &["detail"]),
    ("conversion_timeout", "tool_timed_out", &["tool", "secs"]),
    ("print_timeout", "spooler", &["secs"]),
    // Handed to the spooler, never confirmed printed (JobState::Unconfirmed).
    ("print_unconfirmed", "left_queue", &[]),
    ("print_unconfirmed", "untracked", &[]),
    ("print_unconfirmed", "tracking_failed", &[]),
    ("print_unconfirmed", "not_verifiable", &[]),
    ("printer_unavailable", "no_printer", &[]),
    (
        "printer_unavailable",
        "ambiguous",
        &["printer", "candidates"],
    ),
    ("unsupported_setting", "duplex", &["printer"]),
    ("unsupported_setting", "paper", &["printer", "paper"]),
    // --- Conversion (engine::Failure reasons) ---
    ("conversion_failed", "cannot_read_input", &["file"]),
    ("conversion_failed", "workdir_failed", &[]),
    ("conversion_failed", "cannot_write_output", &[]),
    ("conversion_failed", "converter_failed", &["tool"]),
    ("conversion_failed", "converter_start_failed", &["tool"]),
    ("conversion_failed", "converter_crashed", &["tool"]),
    ("conversion_failed", "converter_no_output", &["tool"]),
    ("conversion_failed", "pdf_invalid", &[]),
    ("conversion_failed", "pdf_truncated", &[]),
    ("conversion_failed", "pdf_no_pages", &[]),
    ("conversion_failed", "pdf_decrypt_failed", &[]),
    ("conversion_failed", "image_damaged", &[]),
    ("conversion_failed", "heif_decode_failed", &[]),
    (
        "conversion_failed",
        "heif_size_unsupported",
        &["width", "height"],
    ),
    // FR-052a (SVG/SVGZ).
    ("conversion_failed", "svg_invalid", &[]),
    ("conversion_failed", "svg_too_large", &["limit"]),
    ("conversion_failed", "svg_convert_failed", &[]),
    // FR-055/056/057 (HTML, Markdown, TXT through the system web view).
    ("conversion_failed", "render_failed", &[]),
    // The render worker and its web view needed more memory than their cap
    // (ADR-0014, round 7).
    ("conversion_failed", "page_too_large", &["limit"]),
    // The console CLI without the app next to it (the app is the renderer).
    ("conversion_failed", "render_worker_missing", &[]),
    // Linux: the worker found no X11 or Wayland session to start WebKitGTK
    // in (a headless CLI run over SSH, a service).
    ("conversion_failed", "render_no_display", &[]),
    // No longer emitted: only the headless-browser fallback, removed in
    // round 6, refused a page whose stylesheets named a share. Kept so an
    // older `queue.json` that carries it still explains itself.
    ("conversion_failed", "share_reference_refused", &[]),
    // A page whose security policy cannot be placed first (its start is
    // malformed, e.g. a doctype that never closes) is not printed.
    ("conversion_failed", "page_not_checkable", &[]),
    ("conversion_failed", "text_unreadable", &[]),
    // --- Printing ---
    ("print_failed", "sumatra_start_failed", &[]),
    ("print_failed", "sumatra_exit", &["status"]),
    ("print_failed", "spooler_rejected", &[]),
    ("print_failed", "wait_failed", &[]),
    // --- Format ---
    ("unsupported_format", "no_extension", &[]),
    ("unsupported_format", "unsupported_type", &["ext"]),
    ("unsupported_format", "gif_not_supported", &[]),
    // --- Protected PDFs (FR-059), see engine::PdfProtection::reason ---
    ("pdf_protected", "password_required", &[]),
    ("pdf_protected", "password_incorrect", &[]),
    ("pdf_protected", "printing_not_allowed", &[]),
    ("pdf_protected", "unsupported_encryption", &["detail"]),
    // --- Dependencies (FR-058): the reason is the dependency id, the
    // sentence is `common:deps.<id>.remediation`. ---
    ("dependency_missing", "sumatra-pdf", &[]),
    ("dependency_missing", "libre-office", &[]),
    ("dependency_missing", "chromium", &[]),
    ("dependency_missing", "pandoc", &[]),
    ("dependency_missing", "heif-codec", &[]),
    ("dependency_missing", "hevc-codec", &[]),
    ("dependency_missing", "heif-decoder", &[]),
    ("dependency_missing", "no-heif-decoder", &[]),
    ("dependency_missing", "sips", &[]),
    ("dependency_missing", "cups", &[]),
    ("dependency_missing", "no-print-backend", &[]),
];

/// Every note the Windows honesty preview can add below the SumatraPDF
/// command ([`crate::engine::print::sumatra::PreviewNote`]), with the
/// parameters its sentence may use. Translated from
/// `printJob:preview.notes.<reason>`; a `quality` value is a keyword from
/// `printJob:preview.qualityWords`.
pub const PREVIEW_NOTES: &[(&str, &[&str])] = &[
    ("quality_offered", &["quality"]),
    ("quality_unknown", &["quality"]),
    ("quality_not_offered", &["quality"]),
    ("quality_pending_restore", &["quality"]),
    ("collated_not_applied", &["copies"]),
    ("uncollated_not_applied", &["copies"]),
    ("paper_not_listed", &["paper"]),
    ("tray_not_listed", &["tray"]),
];

/// The `conversion_failed` reasons that are deterministic: the same file on
/// the same machine fails the same way every time, so the queue fails the job
/// at once instead of retrying it (FR-034 retries are for transient failures).
pub const TERMINAL_CONVERSION_REASONS: &[&str] = &[
    "render_worker_missing",
    "render_no_display",
    "share_reference_refused",
    "page_not_checkable",
    "svg_too_large",
    "page_too_large",
    "heif_size_unsupported",
];

/// Whether a `conversion_failed` `reason` is terminal
/// ([`TERMINAL_CONVERSION_REASONS`]).
pub fn is_terminal_conversion_reason(reason: &str) -> bool {
    TERMINAL_CONVERSION_REASONS.contains(&reason)
}

/// Whether `reason` is a registered preview note.
pub fn is_preview_note(reason: &str) -> bool {
    PREVIEW_NOTES.iter().any(|(r, _)| *r == reason)
}

/// The job error `code` an engine failure of this variant is recorded under.
pub fn job_code_of(err: &crate::engine::EngineError) -> &'static str {
    use crate::engine::EngineError as E;
    match err {
        E::ConversionFailed(_) => "conversion_failed",
        E::ConversionTimedOut(_) => "conversion_timeout",
        E::PrintFailed(_) => "print_failed",
        E::UnsupportedSetting { .. } => "unsupported_setting",
        E::DependencyMissing(_) => "dependency_missing",
        E::UnsupportedFormat(_) => "unsupported_format",
        E::PdfProtected(_) => "pdf_protected",
        E::Cancelled => "cancelled",
    }
}

/// The IPC family an engine job code surfaces as when the failure crosses the
/// IPC boundary (the CLI, a command) instead of landing on a queued job.
pub fn ipc_family_of_job_code(code: &str) -> Option<IpcErrorCode> {
    Some(match code {
        "conversion_failed" | "conversion_timeout" => IpcErrorCode::ConversionFailed,
        "print_failed" => IpcErrorCode::PrintFailed,
        "dependency_missing" => IpcErrorCode::DependencyMissing,
        "unsupported_format" => IpcErrorCode::UnsupportedFormat,
        "pdf_protected" => IpcErrorCode::PdfProtected,
        _ => return None,
    })
}

/// Whether `reason` is registered for the IPC family `code`, directly or as an
/// engine reason of a job code that surfaces in that family.
pub fn is_ipc_reason(code: IpcErrorCode, reason: &str) -> bool {
    IPC_REASONS
        .iter()
        .any(|(c, r, _)| *c == code && *r == reason)
        || JOB_REASONS
            .iter()
            .any(|(c, r, _)| *r == reason && ipc_family_of_job_code(c) == Some(code))
}

/// Whether `reason` is registered for the job error `code`.
pub fn is_job_reason(code: &str, reason: &str) -> bool {
    JOB_REASONS
        .iter()
        .any(|(c, r, _)| *c == code && *r == reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const LOCALES: &[(&str, &str, &str)] = &[
        (
            "en",
            include_str!("../../../src/i18n/locales/en/common.json"),
            include_str!("../../../src/i18n/locales/en/queue.json"),
        ),
        (
            "es",
            include_str!("../../../src/i18n/locales/es/common.json"),
            include_str!("../../../src/i18n/locales/es/queue.json"),
        ),
    ];

    fn code_name(code: IpcErrorCode) -> String {
        serde_json::to_value(code)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    fn lookup<'a>(root: &'a Value, path: &[&str]) -> Option<&'a Value> {
        path.iter().try_fold(root, |v, k| v.get(*k))
    }

    /// The sentences of `reason` under `parent`: the reason itself, or its
    /// plural pair.
    fn sentences(parent: Option<&Value>, reason: &str) -> Vec<String> {
        let Some(parent) = parent else {
            return Vec::new();
        };
        if let Some(s) = parent.get(reason).and_then(Value::as_str) {
            return vec![s.to_string()];
        }
        [format!("{reason}_one"), format!("{reason}_other")]
            .iter()
            .filter_map(|k| parent.get(k).and_then(Value::as_str))
            .map(str::to_string)
            .collect()
    }

    fn placeholders(sentence: &str) -> Vec<String> {
        sentence
            .split("{{")
            .skip(1)
            .filter_map(|rest| rest.split_once("}}"))
            .map(|(name, _)| name.trim().to_string())
            .collect()
    }

    fn check(lang: &str, key: &str, found: Vec<String>, params: &[&str], errors: &mut Vec<String>) {
        if found.is_empty() || found.iter().any(|s| s.trim().is_empty()) {
            errors.push(format!("{lang}: no sentence for {key}"));
        }
        for s in &found {
            for p in placeholders(s) {
                if !params.contains(&p.as_str()) {
                    errors.push(format!("{lang}: {key} uses undeclared {{{{{p}}}}}"));
                }
            }
        }
    }

    /// FR-092: every reason Rust can emit has a sentence in both languages,
    /// and every placeholder in those sentences is a declared parameter.
    #[test]
    fn fr_092_every_rust_reason_has_es_en_phrase_and_declared_params() {
        let mut errors = Vec::new();
        for (lang, common, queue) in LOCALES {
            let common: Value = serde_json::from_str(common).unwrap();
            let queue: Value = serde_json::from_str(queue).unwrap();
            for (code, reason, params) in IPC_REASONS {
                let code = code_name(*code);
                let parent = lookup(&common, &["ipcErrors", "reasons", &code]);
                let key = format!("common:ipcErrors.reasons.{code}.{reason}");
                check(lang, &key, sentences(parent, reason), params, &mut errors);
            }
            for (code, reason, params) in JOB_REASONS {
                let (key, found) = if *code == "dependency_missing" {
                    let parent = lookup(&common, &["deps", reason]);
                    (
                        format!("common:deps.{reason}.remediation"),
                        sentences(parent, "remediation"),
                    )
                } else {
                    let parent = lookup(&queue, &["jobErrors", "reasons", code]);
                    (
                        format!("queue:jobErrors.reasons.{code}.{reason}"),
                        sentences(parent, reason),
                    )
                };
                check(lang, &key, found, params, &mut errors);
            }
            // Every engine job code has a family sentence to fall back to.
            for code in [
                "conversion_failed",
                "conversion_timeout",
                "print_failed",
                "dependency_missing",
                "unsupported_format",
                "pdf_protected",
            ] {
                if lookup(&queue, &["jobErrors", "codes", code]).is_none() {
                    errors.push(format!("{lang}: no queue:jobErrors.codes.{code}"));
                }
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    /// Every IPC family has its own sentence (`common:ipcErrors.codes.<Code>`).
    #[test]
    fn fr_092_every_ipc_family_has_a_code_sentence() {
        for (lang, common, _) in LOCALES {
            let common: Value = serde_json::from_str(common).unwrap();
            for code in crate::util::error::IpcErrorCode::ALL {
                let name = code_name(*code);
                assert!(
                    lookup(&common, &["ipcErrors", "codes", &name]).is_some(),
                    "{lang}: no common:ipcErrors.codes.{name}"
                );
            }
        }
    }

    /// FR-092 / D16: every honesty-preview note has a sentence in both
    /// languages that uses only its declared parameters, and every quality
    /// keyword it may carry is translated too.
    #[test]
    fn fr_092_every_preview_note_has_es_en_phrase_and_declared_params() {
        let catalogs = [
            (
                "en",
                include_str!("../../../src/i18n/locales/en/printJob.json"),
            ),
            (
                "es",
                include_str!("../../../src/i18n/locales/es/printJob.json"),
            ),
        ];
        let mut errors = Vec::new();
        for (lang, raw) in catalogs {
            let doc: Value = serde_json::from_str(raw).unwrap();
            let notes = lookup(&doc, &["preview", "notes"]);
            for (reason, params) in PREVIEW_NOTES {
                let key = format!("printJob:preview.notes.{reason}");
                check(lang, &key, sentences(notes, reason), params, &mut errors);
            }
            for word in ["draft", "high"] {
                if lookup(&doc, &["preview", "qualityWords", word]).is_none() {
                    errors.push(format!("{lang}: no printJob:preview.qualityWords.{word}"));
                }
            }
            if lookup(&doc, &["preview", "notePrefix"]).is_none() {
                errors.push(format!("{lang}: no printJob:preview.notePrefix"));
            }
            // Nothing in the catalog the registry does not know about.
            if let Some(Value::Object(map)) = notes {
                for k in map.keys() {
                    if !is_preview_note(k) {
                        errors.push(format!("{lang}: unregistered preview note {k}"));
                    }
                }
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    /// Every terminal conversion reason is a registered one.
    #[test]
    fn fr_034_terminal_conversion_reasons_are_registered() {
        for reason in TERMINAL_CONVERSION_REASONS {
            assert!(is_job_reason("conversion_failed", reason), "{reason}");
        }
        assert!(!is_terminal_conversion_reason("render_failed"));
    }

    #[test]
    fn fr_092_registry_has_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for (c, r, _) in IPC_REASONS {
            assert!(seen.insert(format!("ipc:{}.{r}", code_name(*c))), "{r}");
        }
        for (c, r, _) in JOB_REASONS {
            assert!(seen.insert(format!("job:{c}.{r}")), "{r}");
        }
    }

    #[test]
    fn fr_092_engine_reasons_are_valid_in_their_ipc_family() {
        assert!(is_ipc_reason(
            IpcErrorCode::ConversionFailed,
            "pdf_truncated"
        ));
        assert!(is_ipc_reason(
            IpcErrorCode::PdfProtected,
            "password_required"
        ));
        assert!(is_ipc_reason(
            IpcErrorCode::DependencyMissing,
            "libre-office"
        ));
        assert!(!is_ipc_reason(IpcErrorCode::PrintFailed, "pdf_truncated"));
        assert!(is_job_reason("pdf_protected", "printing_not_allowed"));
        assert!(!is_job_reason("print_failed", "password_required"));
    }
}
