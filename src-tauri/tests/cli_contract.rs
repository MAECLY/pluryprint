//! The CLI's public contract: argv → command, exit codes, `--help`, the
//! handoff file and the single-instance argv (FR-008/009/010/112, NFR-014).
//!
//! Everything here is the *surface* a script or a shell extension depends on,
//! so these tests are deliberately written as assertions about the contract
//! rather than about the implementation: an exit code that changes value, a
//! flag that disappears, a `--help` that goes missing or a handoff payload that
//! stops being validated all fail here.
//!
//! They are hermetic by construction — argv parsing, pure validation and
//! tempdir-backed stores only. Nothing enumerates a printer, spools a job or
//! reads the developer's real configuration.

use clap::Parser;

use pluryprint_lib::cli::args::{
    BridgeCmd, Cli, CliBrowser, CliChannel, CliDuplex, CliScale, Command, ContextAction,
    InvokeSource, PrintersCmd, ProfilesCmd, QueueCmd, UpdateCmd, SUBCOMMANDS,
};
use pluryprint_lib::cli::exit::{exit_code_for, CliError, ExitCode};
use pluryprint_lib::cli::handoff::{
    self, HANDOFF_SCHEMA_VERSION, MAX_HANDOFF_AGE, MAX_HANDOFF_PATHS,
};
use pluryprint_lib::cli::output;
use pluryprint_lib::cli::{classify, pending, selection_from_payload, single_instance, Intent};
use pluryprint_lib::queue::job::{now_rfc3339, PaperSize};
use pluryprint_lib::util::error::IpcError;

/// Parse an argv the way `main` does (program name included).
fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    let mut argv = vec!["pluryprint"];
    argv.extend_from_slice(args);
    Cli::try_parse_from(argv)
}

// ---------------------------------------------------------------------------
// FR-008: every subcommand and flag parses to what it says on the tin
// ---------------------------------------------------------------------------

#[test]
fn fr_008_print_accepts_the_documented_flags() {
    let cli = parse(&[
        "print",
        "--files",
        "a.pdf,b.png",
        "--profile",
        "Duplex BW",
        "--printer",
        "Front Desk",
        "--mono",
        "--duplex",
        "long-edge",
        "--copies",
        "3",
        "--paper",
        "letter",
        "--range",
        "1-5,8",
        "--scale",
        "actual",
        "--orientation",
        "landscape",
        "--no-dialog",
        "--wait",
    ])
    .expect("the documented print invocation must parse");

    let Some(Command::Print(args)) = cli.command else {
        panic!("expected the print subcommand");
    };
    // A comma-separated list is one flag, N files (§3.3).
    assert_eq!(args.files, vec!["a.pdf", "b.png"]);
    assert_eq!(args.profile.as_deref(), Some("Duplex BW"));
    assert_eq!(args.printer.as_deref(), Some("Front Desk"));
    assert!(args.settings.mono && !args.settings.color);
    assert_eq!(args.settings.duplex, Some(CliDuplex::LongEdge));
    assert_eq!(args.settings.copies, Some(3));
    assert_eq!(args.settings.paper, Some(PaperSize::LETTER));
    assert_eq!(args.settings.range.as_deref(), Some("1-5,8"));
    assert_eq!(args.settings.scale, Some(CliScale::Actual));
    assert!(args.no_dialog && args.wait);
}

#[test]
fn fr_008_print_takes_files_or_files_from_but_not_neither() {
    // Neither: rejected by the argument group, not by a runtime check.
    assert!(parse(&["print"]).is_err());
    // Repeated `--files` and a handoff file are both legal on their own.
    assert!(parse(&["print", "--files", "a.pdf", "--files", "b.pdf"]).is_ok());
    assert!(parse(&["print", "--files-from", "/tmp/sel.json"]).is_ok());
}

/// FR-059 / NFR-082: the PDF password comes from a file or stdin, never from
/// argv (where `ps` and the shell history would show it).
#[test]
fn fr_059_password_is_read_from_a_file_or_stdin_never_argv() {
    let cli = parse(&["print", "--files", "a.pdf", "--password-file", "/tmp/pw"]).unwrap();
    let Some(Command::Print(args)) = cli.command else {
        panic!("expected the print subcommand");
    };
    assert_eq!(
        args.password_file.as_deref(),
        Some(std::path::Path::new("/tmp/pw"))
    );
    assert!(!args.password_stdin);

    let cli = parse(&["print", "--files", "a.pdf", "--password-stdin"]).unwrap();
    let Some(Command::Print(args)) = cli.command else {
        panic!("expected the print subcommand");
    };
    assert!(args.password_stdin && args.password_file.is_none());

    // One source at a time, and no flag takes the password itself.
    assert!(parse(&[
        "print",
        "--files",
        "a.pdf",
        "--password-file",
        "/tmp/pw",
        "--password-stdin"
    ])
    .is_err());
    for flag in ["--password", "--pwd", "--pass"] {
        assert!(
            parse(&["print", "--files", "a.pdf", flag, "hunter2"]).is_err(),
            "{flag} must not exist"
        );
    }
}

/// FR-059: a protected PDF exits 13, whichever reason.
#[test]
fn fr_059_protected_pdf_exit_code_is_13() {
    assert_eq!(ExitCode::PdfProtected.code(), 13);
}

#[test]
fn fr_008_color_and_mono_are_mutually_exclusive() {
    assert!(parse(&["print", "--files", "a.pdf", "--color"]).is_ok());
    assert!(parse(&["print", "--files", "a.pdf", "--mono"]).is_ok());
    assert!(parse(&["print", "--files", "a.pdf", "--color", "--mono"]).is_err());
}

#[test]
fn fr_008_value_vocabularies_are_checked_and_case_insensitive() {
    assert!(parse(&["print", "--files", "a.pdf", "--paper", "A4"]).is_ok());
    assert!(parse(&["print", "--files", "a.pdf", "--paper", "a4"]).is_ok());
    // Paper is open: any size a printer reports is a valid name, checked
    // against the target printer before spooling (FR-041). Blank is not.
    assert!(parse(&["print", "--files", "a.pdf", "--paper", "A6"]).is_ok());
    assert!(parse(&["print", "--files", "a.pdf", "--paper", "auto"]).is_ok());
    assert!(parse(&["print", "--files", "a.pdf", "--paper", ""]).is_err());
    assert!(parse(&["print", "--files", "a.pdf", "--duplex", "sideways"]).is_err());
    assert!(parse(&["print", "--files", "a.pdf", "--scale", "none"]).is_err());
}

#[test]
fn fr_008_the_other_four_subcommands_parse() {
    assert!(matches!(
        parse(&["printers", "list"]).unwrap().command,
        Some(Command::Printers {
            cmd: PrintersCmd::List
        })
    ));
    assert!(matches!(
        parse(&["profiles", "list"]).unwrap().command,
        Some(Command::Profiles {
            cmd: ProfilesCmd::List
        })
    ));
    assert!(matches!(
        parse(&["profiles", "show", "Quick"]).unwrap().command,
        Some(Command::Profiles {
            cmd: ProfilesCmd::Show { .. }
        })
    ));
    assert!(matches!(
        parse(&["profiles", "set-default", "builtin-quick"])
            .unwrap()
            .command,
        Some(Command::Profiles {
            cmd: ProfilesCmd::SetDefault { .. }
        })
    ));
    assert!(matches!(
        parse(&["profiles", "delete", "c-1"]).unwrap().command,
        Some(Command::Profiles {
            cmd: ProfilesCmd::Delete { .. }
        })
    ));
    assert!(matches!(
        parse(&["queue", "status"]).unwrap().command,
        Some(Command::Queue {
            cmd: QueueCmd::Status { watch: false }
        })
    ));
    assert!(matches!(
        parse(&["queue", "status", "--watch"]).unwrap().command,
        Some(Command::Queue {
            cmd: QueueCmd::Status { watch: true }
        })
    ));
    assert!(matches!(
        parse(&["queue", "cancel", "job-1"]).unwrap().command,
        Some(Command::Queue {
            cmd: QueueCmd::Cancel { .. }
        })
    ));
    assert!(matches!(
        parse(&["queue", "cancel-all"]).unwrap().command,
        Some(Command::Queue {
            cmd: QueueCmd::CancelAll
        })
    ));
    assert!(matches!(
        parse(&["update", "check"]).unwrap().command,
        Some(Command::Update {
            cmd: UpdateCmd::Check
        })
    ));
    assert!(matches!(
        parse(&["update", "install", "--yes"]).unwrap().command,
        Some(Command::Update {
            cmd: UpdateCmd::Install { yes: true }
        })
    ));
    assert_eq!(
        parse(&["update", "channel", "beta"]).unwrap().command,
        Some(Command::Update {
            cmd: UpdateCmd::Channel {
                channel: Some(CliChannel::Beta)
            }
        })
    );
    assert_eq!(
        parse(&["update", "channel"]).unwrap().command,
        Some(Command::Update {
            cmd: UpdateCmd::Channel { channel: None }
        })
    );
}

/// FR-127: `bridge install|uninstall|status` is a headless subcommand like
/// the others; `--browser` defaults to every browser, `--extra-origin`
/// repeats, and `--json` works before or after it. It adds no exit code.
#[test]
fn fr_127_the_bridge_subcommand_parses() {
    assert_eq!(classify(&["bridge", "status"]), Intent::Headless);
    assert!(SUBCOMMANDS.contains(&"bridge"));
    assert_eq!(
        parse(&["bridge", "install"]).unwrap().command,
        Some(Command::Bridge {
            cmd: BridgeCmd::Install {
                browser: CliBrowser::All,
                extra_origin: vec![],
            }
        })
    );
    let origin = format!("chrome-extension://{}/", "a".repeat(32));
    assert_eq!(
        parse(&[
            "bridge",
            "install",
            "--browser",
            "Edge",
            "--extra-origin",
            &origin,
            "--extra-origin",
            &origin,
        ])
        .unwrap()
        .command,
        Some(Command::Bridge {
            cmd: BridgeCmd::Install {
                browser: CliBrowser::Edge,
                extra_origin: vec![origin.clone(), origin.clone()],
            }
        })
    );
    assert_eq!(
        parse(&["bridge", "uninstall", "--browser", "brave"])
            .unwrap()
            .command,
        Some(Command::Bridge {
            cmd: BridgeCmd::Uninstall {
                browser: CliBrowser::Brave
            }
        })
    );
    let cli = parse(&["bridge", "status", "--json"]).unwrap();
    assert!(cli.json);
    assert_eq!(
        cli.command,
        Some(Command::Bridge {
            cmd: BridgeCmd::Status
        })
    );
    assert!(parse(&["--json", "bridge", "status"]).unwrap().json);
    assert!(parse(&["bridge", "install", "--browser", "firefox"]).is_err());
    assert!(parse(&["bridge"]).is_err(), "an action is required");
}

#[test]
fn fr_008_profiles_create_carries_the_full_setting_surface() {
    let cli = parse(&[
        "profiles",
        "create",
        "--name",
        "Press",
        "--from",
        "Photo Color",
        "--printer",
        "Press A",
        "--quality",
        "high",
        "--tray",
        "Tray 2",
        "--color",
        "--copies",
        "2",
    ])
    .expect("profiles create must accept the documented flags");

    let Some(Command::Profiles {
        cmd:
            ProfilesCmd::Create {
                name,
                from,
                printer,
                tray,
                settings,
                ..
            },
    }) = cli.command
    else {
        panic!("expected profiles create");
    };
    assert_eq!(name, "Press");
    assert_eq!(from.as_deref(), Some("Photo Color"));
    assert_eq!(printer.as_deref(), Some("Press A"));
    assert_eq!(tray.as_deref(), Some("Tray 2"));
    assert!(settings.color);
    assert_eq!(settings.copies, Some(2));
}

#[test]
fn fr_010_json_is_accepted_before_and_after_the_subcommand() {
    assert!(parse(&["--json", "printers", "list"]).unwrap().json);
    assert!(parse(&["printers", "list", "--json"]).unwrap().json);
    assert!(!parse(&["printers", "list"]).unwrap().json);
}

#[test]
fn nfr_014_context_menu_argv_parses_to_a_handoff_request() {
    let cli = parse(&[
        "--invoke=context-menu",
        "--files-from",
        "/tmp/handoff/sel.json",
        "--action",
        "print-default",
        "--profile",
        "Quick",
    ])
    .expect("the shell-extension invocation must parse");

    assert_eq!(cli.invoke, Some(InvokeSource::ContextMenu));
    assert_eq!(
        cli.files_from.as_deref(),
        Some(std::path::Path::new("/tmp/handoff/sel.json"))
    );
    assert_eq!(cli.action, ContextAction::PrintDefault);
    assert_eq!(cli.profile.as_deref(), Some("Quick"));
    assert!(cli.command.is_none(), "no subcommand is involved");
}

// ---------------------------------------------------------------------------
// FR-112: `--help` everywhere
// ---------------------------------------------------------------------------

#[test]
fn fr_112_every_subcommand_renders_a_non_empty_help() {
    use clap::CommandFactory;

    /// Walk the whole command tree, asserting each node has usable help.
    fn assert_help(cmd: &mut clap::Command, path: &str) {
        let help = cmd.render_long_help().to_string();
        assert!(
            help.len() > 20,
            "`{path} --help` rendered nothing usable: {help:?}"
        );
        assert!(
            help.contains("Usage:"),
            "`{path} --help` has no usage line: {help}"
        );
        let children: Vec<String> = cmd
            .get_subcommands()
            .map(|s| s.get_name().to_string())
            .collect();
        for name in children {
            let child = cmd
                .find_subcommand_mut(&name)
                .expect("subcommand just enumerated");
            assert_help(child, &format!("{path} {name}"));
        }
    }

    assert_help(&mut Cli::command(), "pluryprint");
}

#[test]
fn fr_112_help_and_version_are_requests_not_errors() {
    use clap::error::ErrorKind;
    for args in [
        vec!["--help"],
        vec!["print", "--help"],
        vec!["profiles", "create", "--help"],
        vec!["queue", "cancel", "--help"],
    ] {
        let err = parse(&args).expect_err("clap reports help as an error kind");
        assert_eq!(err.kind(), ErrorKind::DisplayHelp, "for {args:?}");
    }
    assert_eq!(
        parse(&["--version"]).unwrap_err().kind(),
        ErrorKind::DisplayVersion
    );
}

#[test]
fn fr_112_top_level_help_lists_every_subcommand() {
    use clap::CommandFactory;
    let help = Cli::command().render_long_help().to_string();
    for sub in SUBCOMMANDS {
        assert!(help.contains(sub), "top-level help omits `{sub}`:\n{help}");
    }
}

// ---------------------------------------------------------------------------
// FR-009: the exit-code table is frozen
// ---------------------------------------------------------------------------

/// The table from `docs/04-architecture/ipc-and-cli.md` §3.7, spelled out.
/// These numbers are what scripts branch on: changing one is a breaking change
/// and must fail here first.
#[test]
fn fr_009_exit_codes_are_the_documented_numbers() {
    assert_eq!(ExitCode::Success.code(), 0);
    assert_eq!(ExitCode::Generic.code(), 1);
    assert_eq!(ExitCode::Usage.code(), 2);
    assert_eq!(ExitCode::Config.code(), 3);
    assert_eq!(ExitCode::Profile.code(), 4);
    assert_eq!(ExitCode::Printer.code(), 5);
    assert_eq!(ExitCode::FileNotFound.code(), 6);
    assert_eq!(ExitCode::UnsupportedFile.code(), 7);
    assert_eq!(ExitCode::ConversionFailed.code(), 8);
    assert_eq!(ExitCode::PrintFailed.code(), 9);
    assert_eq!(ExitCode::UpdateCheckFailed.code(), 10);
    assert_eq!(ExitCode::UpdateInstallFailed.code(), 11);
    assert_eq!(ExitCode::Cancelled.code(), 12);
    assert_eq!(ExitCode::PdfProtected.code(), 13);
    // §3.7 reserves 64+ for sysexits.h; nothing we return may reach it.
    for code in [
        ExitCode::Success,
        ExitCode::Generic,
        ExitCode::Usage,
        ExitCode::Config,
        ExitCode::Profile,
        ExitCode::Printer,
        ExitCode::FileNotFound,
        ExitCode::UnsupportedFile,
        ExitCode::ConversionFailed,
        ExitCode::PrintFailed,
        ExitCode::UpdateCheckFailed,
        ExitCode::UpdateInstallFailed,
        ExitCode::Cancelled,
        ExitCode::PdfProtected,
    ] {
        assert!(code.code() < 64, "{code:?} collides with sysexits.h");
    }
}

#[test]
fn fr_009_every_ipc_error_maps_to_a_stable_code() {
    let cases = [
        (
            IpcError::FileNotFound("a.pdf".into()),
            ExitCode::FileNotFound,
        ),
        (IpcError::InvalidRequest("bad".into()), ExitCode::Usage),
        (
            IpcError::UnsupportedFormat("a.docx".into()),
            ExitCode::UnsupportedFile,
        ),
        (
            IpcError::ConversionFailed("x".into()),
            ExitCode::ConversionFailed,
        ),
        (IpcError::PrintFailed("x".into()), ExitCode::PrintFailed),
        (
            IpcError::UnsupportedSetting("duplex".into()),
            ExitCode::Printer,
        ),
        (IpcError::DependencyMissing("lp".into()), ExitCode::Config),
        (IpcError::JobNotFound("j".into()), ExitCode::Generic),
        (IpcError::InvalidState("done".into()), ExitCode::Generic),
        (IpcError::ProfileNotFound("p".into()), ExitCode::Profile),
        (
            IpcError::UpdateCheckFailed("offline".into()),
            ExitCode::UpdateCheckFailed,
        ),
        (
            IpcError::UpdateInstallFailed("disk full".into()),
            ExitCode::UpdateInstallFailed,
        ),
        (
            IpcError::UpdateSignatureInvalid("tampered".into()),
            ExitCode::UpdateInstallFailed,
        ),
        (IpcError::Internal("boom".into()), ExitCode::Generic),
    ];
    for (err, expected) in cases {
        assert_eq!(exit_code_for(&err), expected, "for {err:?}");
        // …and the `?`-conversion agrees with the explicit mapping.
        assert_eq!(CliError::from(err).exit, expected);
    }
}

/// Drives the real accumulator `cli::print::run` feeds every file into, rather
/// than re-folding `ExitCode::worst` (which is already pinned as a unit test in
/// `cli::exit`). Spooling is what needs a printer; *recording* what the spooler
/// said does not, so the batch rule is exercised here for real: every file keeps
/// a row, and the worst code — not the last, and not the first — is what the
/// process returns.
#[test]
fn fr_009_a_batch_returns_its_worst_code() {
    use pluryprint_lib::cli::print::BatchOutcomes;

    let mut batch = BatchOutcomes::with_capacity(4);
    assert_eq!(
        batch.worst(),
        ExitCode::Success,
        "a batch with nothing wrong exits 0"
    );

    // A batch of four: two files print, one is unsupported, one fails to spool.
    batch.record("/tmp/a.pdf", "Front Desk".into(), Ok(Some("fd-1".into())));
    batch.record(
        "/tmp/b.docx",
        "Front Desk".into(),
        Err(CliError::from(IpcError::UnsupportedFormat(
            "/tmp/b.docx".into(),
        ))),
    );
    batch.record(
        "/tmp/c.pdf",
        "Front Desk".into(),
        Err(CliError::from(IpcError::PrintFailed(
            "spooler said no".into(),
        ))),
    );
    batch.record("/tmp/d.pdf", "Front Desk".into(), Ok(None));

    // UnsupportedFile is 7, PrintFailed is 9: the worst wins even though the
    // batch ended on a success.
    assert_eq!(batch.worst(), ExitCode::PrintFailed);

    let files = batch.into_files();
    assert_eq!(files.len(), 4, "no file is dropped from the report");
    assert_eq!(files[0].state, "done");
    assert_eq!(files[1].state, "failed");
    assert_eq!(files[1].error.as_ref().unwrap().code, "UnsupportedFormat");
    assert_eq!(files[2].error.as_ref().unwrap().code, "PrintFailed");
    assert_eq!(files[3].state, "done");
    assert!(files[3].os_job_id.is_none(), "a backend may report no id");
}

// ---------------------------------------------------------------------------
// FR-010: machine-readable output
// ---------------------------------------------------------------------------

#[test]
fn fr_010_errors_serialize_as_the_documented_envelope() {
    let json = output::error_json(&IpcError::FileNotFound("C:\\a.pdf".into()));
    let v: serde_json::Value = serde_json::from_str(&json).expect("stderr JSON must parse");
    assert_eq!(v["error"]["code"], "FileNotFound");
    assert_eq!(v["error"]["message"], "C:\\a.pdf");
}

#[test]
fn fr_010_print_report_is_a_stable_json_object() {
    use pluryprint_lib::cli::print::{FileOutcome, PrintReport};
    use pluryprint_lib::queue::job::{JobError, JobSettings};

    let report = PrintReport {
        batch_id: "b-1".to_string(),
        printer: "Front Desk".to_string(),
        profile_name: Some("Quick".to_string()),
        settings: JobSettings::default(),
        files: vec![
            FileOutcome {
                path: "/tmp/a.pdf".to_string(),
                printer: "Front-Desk".to_string(),
                state: "done".to_string(),
                os_job_id: Some("Front-Desk-42".to_string()),
                error: None,
            },
            FileOutcome {
                path: "/tmp/b.pdf".to_string(),
                printer: "Front-Desk".to_string(),
                state: "failed".to_string(),
                os_job_id: None,
                error: Some(JobError::new("PrintFailed", "spooler said no")),
            },
        ],
        exit_code: ExitCode::PrintFailed.code(),
    };

    let v: serde_json::Value =
        serde_json::from_str(&output::to_json(&report)).expect("report must be valid JSON");
    assert_eq!(v["batch_id"], "b-1");
    assert_eq!(v["printer"], "Front Desk");
    assert_eq!(v["exit_code"], 9);
    assert_eq!(v["files"][0]["state"], "done");
    assert_eq!(v["files"][1]["error"]["code"], "PrintFailed");
    // The settings ride along in the same lowercase wire vocabulary the UI uses.
    assert_eq!(v["settings"]["duplex"], "none");
    assert_eq!(v["settings"]["paper_size"], "a4");
}

// ---------------------------------------------------------------------------
// The handoff file (shell-extension contract)
// ---------------------------------------------------------------------------

fn payload_json(paths: &[&str], invoked_at: &str) -> String {
    let quoted: Vec<String> = paths
        .iter()
        .map(|p| serde_json::Value::String((*p).to_string()).to_string())
        .collect();
    format!(
        r#"{{"schema_version":{HANDOFF_SCHEMA_VERSION},"source":"windows-explorer",
            "action":"print-default","profile_id":"builtin-quick",
            "invoked_at":"{invoked_at}","paths":[{}]}}"#,
        quoted.join(",")
    )
}

#[test]
fn fr_008_handoff_round_trips_a_real_selection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sel.json");
    std::fs::write(
        &path,
        payload_json(
            &[r"C:\Facturas\2026-01.pdf", r#"C:\Facturas\a, b "c".png"#],
            &now_rfc3339(),
        ),
    )
    .unwrap();

    let payload = handoff::read_file(&path).expect("a well-formed payload is accepted");
    assert_eq!(payload.paths.len(), 2);
    assert_eq!(payload.paths[1], r#"C:\Facturas\a, b "c".png"#);

    let selection = selection_from_payload(payload, ContextAction::Configure, None);
    // The payload's own action/profile win over the argv fallbacks.
    assert_eq!(selection.action, ContextAction::PrintDefault);
    assert_eq!(selection.profile_id.as_deref(), Some("builtin-quick"));
}

#[test]
fn fr_008_handoff_argv_fallbacks_apply_when_the_payload_is_silent() {
    let payload = handoff::parse(
        r#"{"schema_version":1,"invoked_at":"2026-07-20T21:11:03Z","paths":["/tmp/a.pdf"]}"#,
    )
    .expect("action and profile_id are optional");
    let selection =
        selection_from_payload(payload, ContextAction::Queue, Some("Press".to_string()));
    assert_eq!(selection.action, ContextAction::Queue);
    assert_eq!(selection.profile_id.as_deref(), Some("Press"));
    // An older writer says nothing about truncation: the selection is whole.
    assert_eq!(selection.truncated_from, None);
}

/// FR-003 / FR-005: when the shell extension had to cut a huge selection down
/// to the handoff cap, the original size reaches the UI instead of the excess
/// disappearing without a word.
#[test]
fn fr_005_a_truncated_selection_says_how_big_it_was() {
    let payload = handoff::parse(
        r#"{"schema_version":1,"invoked_at":"2026-07-20T21:11:03Z",
            "paths":["/tmp/a.pdf"],"truncated_from":12000}"#,
    )
    .expect("truncated_from is part of the schema");
    assert_eq!(payload.truncated_from, Some(12_000));
    let selection = selection_from_payload(payload, ContextAction::Configure, None);
    assert_eq!(selection.truncated_from, Some(12_000));
}

#[test]
fn fr_009_handoff_failures_are_usage_errors_except_a_missing_file() {
    // Corrupt JSON.
    assert_eq!(handoff::parse("{oops").unwrap_err().exit, ExitCode::Usage);
    // Wrong schema version.
    assert_eq!(
        handoff::parse(r#"{"schema_version":2,"paths":["/a.pdf"]}"#)
            .unwrap_err()
            .exit,
        ExitCode::Usage
    );
    // Empty selection.
    assert_eq!(
        handoff::parse(r#"{"schema_version":1,"paths":[]}"#)
            .unwrap_err()
            .exit,
        ExitCode::Usage
    );
    // A path list past the hard cap.
    let many: Vec<String> = (0..=MAX_HANDOFF_PATHS)
        .map(|i| format!("\"/tmp/f{i}.pdf\""))
        .collect();
    let over = format!(r#"{{"schema_version":1,"paths":[{}]}}"#, many.join(","));
    assert_eq!(handoff::parse(&over).unwrap_err().exit, ExitCode::Usage);

    // A path that is not there at all is a file error, not a usage error.
    let err = handoff::read_file(std::path::Path::new("/no/such/sel.json")).unwrap_err();
    assert_eq!(err.exit, ExitCode::FileNotFound);
}

#[test]
fn fr_008_handoff_carries_a_selection_argv_could_not() {
    // The reason the file exists: Windows caps a command line at 32,767
    // characters, and this selection is far past that.
    let paths: Vec<String> = (0..2_000)
        .map(|i| format!(r"C:\Users\ana\Facturas\2026-invoice-{i:05}.pdf"))
        .collect();
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    let raw = payload_json(&refs, &now_rfc3339());
    assert!(
        raw.len() > 32_767,
        "the fixture must be bigger than a Windows command line"
    );

    let payload = handoff::parse(&raw).expect("2,000 files is a legal selection");
    assert_eq!(payload.paths.len(), 2_000);
}

#[test]
fn nfr_014_stale_or_foreign_payloads_are_refused() {
    let now = time::OffsetDateTime::now_utc();
    assert!(handoff::is_fresh(&now_rfc3339(), now, MAX_HANDOFF_AGE));
    assert!(!handoff::is_fresh(
        "2020-01-01T00:00:00Z",
        now,
        MAX_HANDOFF_AGE
    ));

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sel.json");
    std::fs::write(&file, "{}").unwrap();
    assert!(handoff::is_within_dir(dir.path(), &file));
    assert!(!handoff::is_within_dir(&dir.path().join("nested"), &file));
}

// ---------------------------------------------------------------------------
// NFR-014: what a forwarded second launch means
// ---------------------------------------------------------------------------

#[test]
fn nfr_014_argv_classification_decides_the_mode() {
    assert_eq!(classify(&["print", "--files", "a.pdf"]), Intent::Headless);
    assert_eq!(classify(&["--cli"]), Intent::Headless);
    assert_eq!(
        classify(&["--invoke=context-menu", "--files-from", "/tmp/s.json"]),
        Intent::ContextMenu
    );
    let none: [&str; 0] = [];
    assert_eq!(classify(&none), Intent::Gui);
    assert_eq!(classify(&["-psn_0_1234"]), Intent::Gui);
}

#[test]
fn nfr_014_a_forwarded_launch_without_a_selection_is_just_a_focus_request() {
    // These are the argvs a second launch realistically carries. None of them
    // may make the running instance print anything.
    assert!(single_instance::interpret(&["pluryprint"], "").is_none());
    assert!(single_instance::interpret(&["pluryprint", "-psn_0_99"], "").is_none());
    assert!(single_instance::interpret(&["pluryprint", "--invoke=context-menu"], "").is_none());
    assert!(single_instance::interpret(
        &[
            "pluryprint",
            "--invoke=context-menu",
            "--files-from",
            "/no/such/sel.json"
        ],
        ""
    )
    .is_none());
}

/// The end-to-end shape of a context-menu launch, asserted across the seam the
/// two halves used to be tested on separately: the shell extension's argv, the
/// file it points at, the selection the app derives from it, and the moment
/// that file is allowed to disappear.
///
/// Written against the *forwarded* reading (the running-instance path) because
/// that is the one that used to fail: the launching process read the payload
/// first, and if reading deleted it the forward found nothing.
#[test]
fn nfr_014_a_forwarded_context_menu_launch_delivers_its_selection() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sel-1234.json");
    std::fs::write(
        &file,
        payload_json(
            &[r"C:\Facturas\2026-01.pdf", r"C:\Facturas\2026-02.pdf"],
            &now_rfc3339(),
        ),
    )
    .unwrap();

    // Exactly what `shell-ext-win` spawns (see its `launch.rs`).
    let argv = [
        "pluryprint".to_string(),
        "--invoke=context-menu".to_string(),
        "--files-from".to_string(),
        file.to_string_lossy().into_owned(),
    ];

    // 1. The launching process reads it — and leaves it alone, because it does
    //    not yet know whether it will be the one showing the window.
    let launching = single_instance::interpret_at(dir.path(), &argv, "")
        .expect("a fresh, contained, well-formed payload is a selection");
    assert_eq!(launching.selection.paths.len(), 2);
    assert_eq!(launching.selection.action, ContextAction::PrintDefault);
    assert!(file.exists(), "reading a handoff file must not consume it");

    // 2. The single-instance plugin forwards the same argv to the running
    //    instance, which reads it again and gets the same selection.
    let running = single_instance::interpret_at(dir.path(), &argv, "")
        .expect("the forwarded argv still resolves");
    assert_eq!(running.selection, launching.selection);
    assert_eq!(running.handoff.as_deref(), Some(file.as_path()));

    // 3. Only once the selection is parked for the window does the file go.
    let pending = pending::park_launch_selection(Some(running.selection), running.handoff);
    assert_eq!(pending.drain().len(), 1, "the window will find it waiting");
    assert!(
        !file.exists(),
        "a delivered selection leaves no payload behind"
    );
    assert!(
        single_instance::interpret_at(dir.path(), &argv, "").is_none(),
        "and the same launch cannot be replayed"
    );
}

#[test]
fn nfr_014_a_forwarded_headless_argv_is_never_acted_on() {
    // A headless run never builds a Tauri app, so it never reaches the
    // single-instance callback; if a malformed launch ever did, the running
    // instance must not execute it.
    assert!(single_instance::interpret(
        &[
            "pluryprint",
            "print",
            "--files",
            "/tmp/a.pdf",
            "--copies",
            "999"
        ],
        ""
    )
    .is_none());
}
