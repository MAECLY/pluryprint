//! Pluryprint Tauri backend.
//!
//! Boots the Tauri window and exposes the backend over typed (tauri-specta)
//! IPC: the print engine (`engine`), the persistent job queue (`queue`),
//! profile and settings persistence (`profiles`, `settings`), OS printer
//! enumeration and management (`platform`), and the GitHub-release update
//! check (`update`).
//!
//! The same backend also has a second, windowless face: the `cli` module. It
//! decides from argv whether this process is a GUI launch at all, so a
//! `pluryprint print …` never reaches [`run_with`] and never creates a webview.

#![deny(missing_docs)]

pub mod bridge;
pub mod cli;
pub mod engine;
pub mod ipc;
pub mod logging;
pub mod platform;
pub mod profiles;
pub mod queue;
pub mod render;
pub mod settings;
pub mod shell_menu;
pub mod update;
pub mod util;
pub mod worker;

use tauri::Manager;

/// Boot the GUI with nothing pre-loaded. Kept as the zero-argument entry point
/// the mobile target expects; desktop `main` calls [`run_with`].
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    run_with(None, None);
}

/// Boot the GUI, optionally pre-loading a file selection handed to us on the
/// command line (a shell-extension launch — see [`cli`]).
///
/// `handoff` is the file that selection was read out of, when there was one. It
/// is still on disk: reading it does not delete it, because this process may
/// yet be intercepted by the single-instance plugin and forwarded to a running
/// instance that has to read the very same file. Whichever process ends up
/// parking the selection is the one that deletes it — here, in `setup`
/// ([`cli::pending::PendingSelections::park_launch`]); there, in the plugin callback
/// below.
///
/// The single-instance plugin is registered **first**, as its documentation
/// requires: a second launch must be intercepted before this process starts
/// recovering `queue.json` or sweeping the conversion cache, which is exactly
/// the work that would destroy the running instance's in-flight jobs
/// (NFR-014).
pub fn run_with(selection: Option<cli::Selection>, handoff: Option<std::path::PathBuf>) {
    // Move what a MaeclyPrint install left behind into the Pluryprint folders
    // (ADR-0009) before anything opens them — the log files included, so this
    // runs first and is logged once logging is up. Its lock is kept until this
    // process holds the instance lock (in `setup`), so a second launch waits
    // for that and then sees a running Pluryprint instead of migrating into
    // the folders of one still starting.
    let util::legacy::Migration {
        report: migration,
        guard: migration_guard,
    } = util::legacy::migrate_current();
    // Initialize structured logging as early as possible so any startup error
    // is captured to disk.
    if let Err(e) = logging::init() {
        eprintln!("logging init failed: {e}");
    }
    util::legacy::log_report(&migration);
    // FR-093: a first run takes the Windows display language now, so the
    // Explorer menu cache written below and the built-in profile names speak
    // it before the window has detected anything (settings::locale).
    settings::locale::settle_first_run();
    // Release builds abort on panic and the Windows GUI has no console: the
    // hook is the only way a crash leaves a trace (NFR-033). Installed right
    // after the log directory exists, before anything that can panic.
    logging::install_panic_hook();

    // Drop handoff files a previous run never consumed (crash between "the
    // shell extension wrote it" and "we read it").
    cli::handoff::sweep(
        &cli::handoff::handoff_dir(),
        cli::handoff::ORPHAN_HANDOFF_AGE,
    );
    // FR-126/FR-127, off the startup path: delete the documents PluryFact
    // sent more than a week ago, and (macOS, where there is no installer)
    // make sure every browser this user has can find the bridge of *this*
    // copy of the app.
    std::thread::spawn(|| {
        bridge::inbox::sweep(
            &bridge::inbox_dir(&util::paths::data_dir()),
            bridge::INBOX_RETENTION,
        );
        if let Some(target) = bridge::register::Target::current() {
            let outcome = bridge::register::register_at_startup(&target);
            if outcome.not_attempted != Some("platform") {
                tracing::info!(
                    event = "bridge_registered_at_startup",
                    written = outcome.written.len(),
                    unchanged = outcome.unchanged.len(),
                    skipped = outcome.skipped.len(),
                    not_attempted = outcome.not_attempted.unwrap_or(""),
                );
            }
        }
    });
    // Put back a printer default a killed print left swapped (D16).
    #[cfg(windows)]
    engine::print::devmode_swap::recover_stale_journals_in_background();

    #[allow(unused_mut)]
    let mut ctx = context();
    // Before the webview exists: keep WebView2 from probing the account for a
    // blank password, a failed logon on every start (`util::chromium`).
    #[cfg(windows)]
    suppress_webview_logon_probe(&ctx.config().identifier);
    // WebView2 ignores WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS once wry passes
    // explicit arguments, and that variable is how the Edge WebDriver asks for
    // a debugging port (e2e/). Fold it into the windows' own arguments.
    #[cfg(windows)]
    if let Some(extra) = std::env::var(util::chromium::WEBVIEW2_ARGS_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        for window in ctx.config_mut().app.windows.iter_mut() {
            let base = window
                .additional_browser_args
                .clone()
                .unwrap_or_else(|| util::chromium::WRY_DEFAULT_ARGS.to_string());
            window.additional_browser_args =
                Some(util::chromium::merge_browser_args(&base, &extra));
        }
        tracing::info!(event = "webview_env_args_applied");
    }

    let builder = specta_builder();

    // Regenerate the typed bindings on every debug launch as a convenience;
    // `cargo test` does the same headlessly (see `export_bindings`), which is
    // what CI and the build pipeline rely on.
    #[cfg(debug_assertions)]
    export_bindings(&builder);

    // Created before the builder, not in `setup`: the single-instance plugin
    // starts listening before our `setup` runs, so a second launch during a
    // cold start must already have somewhere to land. The callback keeps one
    // handle, `setup` manages another — one shared store (NFR-014).
    let pending = cli::pending::PendingSelections::default();
    let forwarded_into = pending.clone();

    // If this launch is about to be forwarded to a running instance, hand that
    // instance the right to come to the front (`cli::foreground`).
    cli::foreground::grant_to_running_instance();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(move |app, argv, cwd| {
            // A second launch already exited by the time this runs; all we get
            // is its argv and working directory. Interpreting them is a pure
            // function of the two and the handoff file the argv names (see
            // `cli::single_instance`), so the only work here is delivery.
            if let Some(forwarded) = cli::single_instance::interpret(&argv, &cwd) {
                use tauri_specta::Event;
                let cli::single_instance::Forwarded { selection, handoff } = forwarded;
                forwarded_into.push(selection.clone());
                // The selection lives in this process now, so the file it
                // travelled in has done its job. Deleting it *here* rather than
                // in the launching process is what lets the same payload be
                // read twice (NFR-014).
                if let Some(handoff) = &handoff {
                    cli::handoff::discard(handoff);
                }
                // Before `setup` there is no window to ring: the selection
                // simply waits for the first drain.
                if let Err(e) = cli::pending::SelectionForwarded(selection).emit(app) {
                    tracing::warn!(event = "selection_forwarded_emit_failed", error = %e);
                }
            }
            // Never `set_focus` here: on Windows its fallback fakes an Alt
            // keystroke that parks the window in menu mode, and the selection
            // then waits for the user's next click (`cli::foreground`).
            if let Some(window) = app.get_webview_window("main") {
                let raised = cli::foreground::bring_to_front(&window);
                tracing::info!(event = "window_raised", outcome = raised.as_str());
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // FR-072: native notifications. Driven Rust-side only
        // (`ipc::notify::notify_native`), so no JS capability is granted.
        .plugin(tauri_plugin_notification::init())
        // FR-082/083: signed updates. Rust-side only (`update::install`); the
        // public key and default endpoint come from `plugins.updater` in
        // tauri.conf.json.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);

            // `[app].launch_minimized`: the config window already exists by the
            // time `setup` runs, so minimize it before the first frame settles.
            // There is no tray icon to hide into, so minimizing is the whole
            // feature: the app stays one click away in the taskbar / Dock, and a
            // second launch un-minimizes it (the single-instance callback
            // above). A launch carrying a selection is never minimized.
            if settings::should_start_minimized(settings::launch_minimized(), selection.is_some()) {
                match app.get_webview_window("main") {
                    Some(window) => {
                        if let Err(e) = window.minimize() {
                            tracing::warn!(event = "start_minimized_failed", error = %e);
                        } else {
                            tracing::info!(event = "started_minimized");
                        }
                    }
                    None => tracing::warn!(event = "start_minimized_no_window"),
                }
            }

            // Anything the command line handed us waits here until the window
            // has mounted and drains it (`take_pending_selections`); parking it
            // is also what retires the handoff file it arrived in.
            pending.park_launch(selection, handoff);
            app.manage(pending);

            // FR-001: the Finder's Services menu ("Print with Pluryprint")
            // reaches the app through this provider, and reaches it in-process
            // — so it must be registered *after* the store above exists, or a
            // service message arriving immediately would have nowhere to land.
            // A registration failure is logged and survived: the app itself is
            // perfectly usable without the menu entry, and refusing to start
            // would be a far worse answer.
            #[cfg(target_os = "macos")]
            if let Err(e) = platform::macos::services::register(app.handle()) {
                tracing::warn!(event = "macos_services_register_failed", error = %e);
            }

            tracing::info!(
                event = "app_started",
                version = env!("CARGO_PKG_VERSION"),
                build_date = env!("BUILD_DATE"),
                "Pluryprint started"
            );

            // Tell the headless CLI that the live queue is in this process'
            // memory (see `cli::instance`): `pluryprint queue cancel` then
            // refuses instead of editing a file we are about to overwrite. The
            // lock is held for the process lifetime by managing it; the OS
            // releases it however the process ends. Failing to take it only
            // costs the CLI that warning, so it is logged, not fatal.
            match cli::instance::claim(&util::paths::data_dir().join(cli::instance::LOCK_FILE)) {
                Ok(Some(lock)) => {
                    app.manage(lock);
                }
                Ok(None) => tracing::warn!(event = "instance_lock_already_held"),
                Err(e) => tracing::warn!(event = "instance_lock_failed", error = %e),
            }
            drop(migration_guard);

            // Boot the print queue: recover persisted state and start the
            // dispatcher, then expose the handle to the IPC commands.
            let queue = queue::spawn(app.handle().clone())
                .map_err(|e| format!("failed to start print queue: {e}"))?;
            app.manage(queue);

            // FR-080: the launch-time update check. It lives here, in `setup`,
            // rather than in a frontend effect, for three reasons: the
            // single-instance plugin above guarantees `setup` runs exactly once
            // per process, so a second launch cannot double-check; the webview
            // can be reloaded or its window closed and reopened, neither of
            // which should re-run a network check; and the preference, the
            // 24h throttle and the skipped-version rule already live in Rust
            // next to the state file, so the frontend would only be able to
            // re-derive them. Everything runs on a detached thread — the sleep
            // included — so a cold start never waits on the network, and a user
            // who quits before the delay elapses simply takes the thread down
            // with the process, having persisted nothing.
            // FR-001: the Explorer menu's localized labels and per-profile
            // entries (Windows only; a no-op elsewhere). Off the startup path:
            // it reads two small files and writes one.
            std::thread::spawn(shell_menu::refresh);

            let update_handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(update::STARTUP_CHECK_DELAY);
                // `None` is every uninteresting outcome (disabled, throttled,
                // offline, up to date, skipped version) — see
                // `update::run_startup_check`.
                let Some(check) = update::run_startup_check() else {
                    return;
                };
                use tauri_specta::Event;
                tracing::info!(event = "update_available", latest = ?check.latest);
                if let Err(e) = ipc::update::UpdateAvailable(check).emit(&update_handle) {
                    tracing::warn!(event = "update_available_emit_failed", error = %e);
                }
            });

            // Resolve the window so failures here surface as setup errors,
            // not silent missing-window bugs.
            if let Some(_main) = app.get_webview_window("main") {
                tracing::info!(event = "main_window_resolved");
            } else {
                tracing::warn!(event = "main_window_missing");
            }
            Ok(())
        })
        .run(ctx)
        .expect("error while running tauri application");
}

/// Stamp the app's WebView2 profile (`%LOCALAPPDATA%\<identifier>\EBWebView`,
/// where Tauri keeps it) so its browser process skips the blank-password
/// logon probe that otherwise records a failed Windows logon on every start
/// and, with the default lockout policy, locks the account (`util::chromium`).
/// A failure only costs that protection, so it is logged, never fatal.
#[cfg(windows)]
fn suppress_webview_logon_probe(identifier: &str) {
    let Some(base) = directories::BaseDirs::new() else {
        return;
    };
    let own = base
        .data_local_dir()
        .join(identifier)
        .join(util::chromium::EBWEBVIEW);
    // A WebView2 started with another profile (the Edge WebDriver passes
    // `--user-data-dir` in WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS) probes from
    // that one, so it is stamped too.
    for dir in std::iter::once(own).chain(util::chromium::extra_profiles_from_env()) {
        if let Err(e) = util::chromium::suppress_blank_password_probe(&dir) {
            tracing::warn!(
                event = "webview_logon_probe_suppress_failed",
                dir = %dir.display(),
                error = %e
            );
        }
    }
}

/// The compiled-in app context (config, assets, icons). One function so the
/// frontend assets are embedded once, however many builders use them.
fn context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
}

/// A windowless Tauri app with only the updater plugin, for the headless
/// `update install` command: the updater needs an `AppHandle` (package info,
/// plugin config) but a CLI run must never open the main window.
///
/// Must be called on the main thread (the event loop is created here even
/// though it is never run), which is where `cli::startup` runs.
pub fn headless_updater_app() -> tauri::Result<tauri::App<tauri::Wry>> {
    let mut ctx = context();
    ctx.config_mut().app.windows.clear();
    #[allow(unused_mut)]
    let mut app = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(ctx)?;
    // No Dock icon for a command-line run.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    Ok(app)
}

/// A windowless Tauri app with no plugins at all, for the hidden render
/// worker (FR-055, D10): it only hosts the WebView2 window that prints a
/// page to PDF ([`render::windows`]). No single-instance, updater, tray or
/// menu, and the render window's label has no IPC capability.
#[cfg(windows)]
pub(crate) fn render_worker_app() -> tauri::Result<tauri::App<tauri::Wry>> {
    let mut ctx = context();
    ctx.config_mut().app.windows.clear();
    tauri::Builder::default().build(ctx)
}

/// The single source of truth for the IPC command surface. Used by both the
/// runtime (`run`) and the binding-export test so they never drift.
fn specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .commands(tauri_specta::collect_commands![
            ipc::diag::app_info,
            ipc::diag::open_logs_folder,
            ipc::diag::report_issue,
            ipc::diag::open_external,
            ipc::diag::log_frontend_error,
            ipc::notify::notify_native,
            ipc::deps_install::install_dependency,
            ipc::deps_install::cancel_dependency_install,
            ipc::deps_install::dependency_install_status,
            ipc::printers::list_printers,
            ipc::printers::default_printer,
            ipc::printers::discover_printers,
            ipc::printers::get_printer_prefs,
            ipc::printers::set_printer_prefs,
            ipc::printers::set_default_printer,
            ipc::printers::print_test_page,
            ipc::printers::open_printer_settings,
            ipc::printers::add_printer,
            ipc::printers::get_printer_policy,
            ipc::jobs::print_files,
            ipc::jobs::queue_status,
            ipc::jobs::cancel_job,
            ipc::jobs::cancel_all,
            ipc::jobs::pause_job,
            ipc::jobs::resume_job,
            ipc::jobs::adjust_job,
            ipc::jobs::retry_job,
            ipc::jobs::pause_queue,
            ipc::jobs::resume_queue,
            ipc::jobs::clear_done,
            ipc::jobs::inspect_files,
            ipc::jobs::expand_folders,
            ipc::jobs::preview_command,
            ipc::pdf_password::job_supply_password,
            ipc::profiles::list_profiles,
            ipc::profiles::get_profile,
            ipc::profiles::save_profile,
            ipc::profiles::delete_profile,
            ipc::profiles::set_default_profile,
            ipc::profiles::export_profile,
            ipc::profiles::import_profiles,
            ipc::settings::get_app_settings,
            ipc::settings::set_app_settings,
            ipc::settings::take_retired_renderer_notice,
            ipc::update::check_for_updates,
            ipc::update::get_update_state,
            ipc::update::set_skipped_update_version,
            ipc::update::install_update,
            ipc::update::restart_app,
            cli::pending::take_pending_selections,
            ipc::bridge::bridge_status,
            ipc::bridge::bridge_reconnect,
        ])
        .events(tauri_specta::collect_events![
            ipc::events::QueueUpdated,
            ipc::events::JobProgressEvent,
            ipc::deps_install::DependencyInstallProgress,
            ipc::update::UpdateAvailable,
            update::install::UpdateDownloadProgress,
            cli::pending::SelectionForwarded
        ])
}

/// Export the TypeScript bindings to `src/ipc/bindings.ts`.
///
/// `@ts-nocheck` disables strict TS checks (notably `noUnusedLocals`) so
/// tauri-specta scaffolding like `__makeEvents__` / `TAURI_CHANNEL` doesn't
/// break CI typecheck. The file is generated, never hand-edited.
///
/// Compiled for tests as well as debug builds so `bindings_are_current` keeps
/// working under `cargo test --release`.
#[cfg(any(debug_assertions, test))]
fn export_bindings(builder: &tauri_specta::Builder<tauri::Wry>) {
    builder
        .export(
            specta_typescript::Typescript::default()
                // `u64` fields (file sizes/byte counts) map to a plain TS
                // `number`. Real print inputs are far below 2^53, so precision is
                // not a concern and the frontend avoids `bigint` ergonomics.
                .bigint(specta_typescript::BigIntExportBehavior::Number)
                .header(
                    "// @ts-nocheck\n/* eslint-disable */\n// AUTO-GENERATED by tauri-specta. Do not edit by hand.\n",
                ),
            "../src/ipc/bindings.ts",
        )
        .expect("failed to export typescript bindings");
}

/// Keeps `src/ipc/bindings.ts` in sync with the Rust command surface. Running
/// `cargo test` regenerates it headlessly (no GUI), which is how CI and the
/// build refresh the typed frontend bindings.
#[cfg(test)]
mod bindings_sync {
    #[test]
    fn bindings_are_current() {
        super::export_bindings(&super::specta_builder());
    }
}
