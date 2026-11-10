//! FR-058: guided installation of a missing dependency (winget, Homebrew, the
//! Microsoft Store or the vendor's web page), always after the user's explicit
//! consent, never silently (decision D13).
//!
//! The UI gets the options from the dependency report
//! ([`crate::engine::deps::DependencyReport::install`]): for each, the exact
//! command that will run (or the page that will open), whether Windows will
//! ask for administrator rights (LibreOffice's MSI) and whether it costs money
//! (HEVC). The consent dialog shows all of it; only its *Install* button sends
//! `consent: true`, and nothing runs without it.
//!
//! - **winget / brew** run with an argument vector (no shell), their output
//!   streamed line by line as [`DependencyInstallProgress`] events, bounded by
//!   [`INSTALL_TIMEOUT`]. One install at a time; the user can stop it
//!   ([`cancel_dependency_install`]), which kills the whole process tree it
//!   can reach. When that cannot be confirmed (an elevated installer that
//!   refuses the kill, or an MSI the Windows Installer service runs outside
//!   the tree) the outcome says so,
//!   [`InstallStatus::CancelRequestedInstallerMayContinue`], never a plain
//!   "stopped"; a timeout the installer may outlive likewise ends in
//!   [`InstallStatus::TimedOutInstallerMayContinue`]. The one-install slot is
//!   then kept until the installer is seen gone, or a grace period ends
//!   ([`linger`]), so a second install never runs beside it.
//! - [`dependency_install_status`] says which install holds the slot, so a
//!   reloaded window finds a running install again instead of offering an
//!   Install the backend would refuse.
//! - **Store / web** open the page with the OS handler; the user finishes
//!   there.
//!
//! When a command finishes the dependency is probed again and the fresh
//! report comes back, so the UI knows at once whether the files it blocked can
//! print now.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use crate::engine::convert::process;
use crate::engine::deps::{self, Dependency, DependencyReport, InstallMethod, InstallTool, Os};
use crate::util::error::{IpcError, IpcErrorCode};

/// How long a winget/brew install may run before it is stopped.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Set while an install runs: one at a time. Only [`InstallSlot`] touches it.
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// Set by [`cancel_dependency_install`]; the running install sees it within a
/// poll and kills its process tree.
static CANCEL: AtomicBool = AtomicBool::new(false);

/// What [`dependency_install_status`] reports: the install holding the slot
/// and how the last one ended. Only [`InstallSlot`] writes it.
static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    running: None,
    last: None,
    seq: 0,
});

struct Registry {
    running: Option<RunningInstall>,
    last: Option<FinishedInstall>,
    seq: u32,
}

fn registry() -> MutexGuard<'static, Registry> {
    // A panic while holding it leaves plain data behind: still usable.
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// How an install ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "kebab-case")]
pub enum InstallStatus {
    /// The command succeeded and the dependency is found now.
    Installed,
    /// The command succeeded but the dependency is still not found (a
    /// restart of Pluryprint, or of Windows, may be needed).
    NotDetected,
    /// The command failed; `exit_code` says how. The web page remains.
    Failed,
    /// A Store or web page was opened; the user finishes there.
    Opened,
    /// The command ran past [`INSTALL_TIMEOUT`] and was stopped, with
    /// everything it started.
    TimedOut,
    /// The command ran past [`INSTALL_TIMEOUT`]; Pluryprint stopped what it
    /// could, but the installer could not be confirmed stopped (see
    /// [`InstallStatus::CancelRequestedInstallerMayContinue`]).
    TimedOutInstallerMayContinue,
    /// The user stopped the command, and everything it started is gone.
    Cancelled,
    /// The user asked to stop, but the installer could not be confirmed
    /// stopped: an elevated part refused the kill, or (a Windows installer
    /// that needs administrator rights) runs in the Windows Installer
    /// service, out of reach. Its window may still be open and it may still
    /// finish.
    CancelRequestedInstallerMayContinue,
}

/// The result of [`install_dependency`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct InstallOutcome {
    /// How it ended.
    pub status: InstallStatus,
    /// The command's exit code, when one ran.
    pub exit_code: Option<i32>,
    /// Whether the installer ran with administrator rights (an elevated
    /// Windows installer), so the UI gives that as the reason a stop may not
    /// have reached it only when it is the reason.
    pub elevated: bool,
    /// The dependency probed again after the install.
    pub report: DependencyReport,
}

impl InstallStatus {
    /// Whether the installer may still be running after this end.
    pub fn may_continue(self) -> bool {
        matches!(
            self,
            Self::CancelRequestedInstallerMayContinue | Self::TimedOutInstallerMayContinue
        )
    }
}

/// The install holding the one-install slot ([`dependency_install_status`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RunningInstall {
    /// Grows with every install this run of Pluryprint starts; matches
    /// [`FinishedInstall::seq`] once it ends.
    pub seq: u32,
    /// The dependency being installed.
    pub id: Dependency,
    /// Its display name.
    pub name: String,
    /// How it is being installed.
    pub method: InstallMethod,
    /// When it started (RFC 3339, UTC).
    pub started_at: String,
    /// Whether the installer runs with administrator rights.
    pub elevated: bool,
    /// The install has ended ([`DependencyInstallState::last`] says how) but
    /// the installer may still be running, so the slot stays taken until it
    /// is seen gone or a grace period ends. Nothing is left to stop.
    pub may_continue: bool,
}

/// How an install ended, for a window that did not see it end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FinishedInstall {
    /// The [`RunningInstall::seq`] it ran under.
    pub seq: u32,
    /// The dependency it installed.
    pub id: Dependency,
    /// How it ended; `None` when it failed before an outcome existed (the
    /// tool could not be started).
    pub outcome: Option<InstallOutcome>,
}

/// The answer of [`dependency_install_status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct DependencyInstallState {
    /// The install holding the slot, if any.
    pub running: Option<RunningInstall>,
    /// The last install that ended since Pluryprint started.
    pub last: Option<FinishedInstall>,
}

/// One line of an install's output, as it happens (FR-058).
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
pub struct DependencyInstallProgress {
    /// The dependency being installed.
    pub id: Dependency,
    /// One line of the installer's output.
    pub line: String,
}

/// What [`plan`] decided to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Open this URL with the OS handler.
    Open(String),
    /// Run `tool` with these arguments.
    Run {
        /// The program.
        tool: InstallTool,
        /// Its argument vector.
        args: Vec<String>,
        /// Whether the installer it starts runs elevated (a UAC prompt), and
        /// so may outlive a cancel ([`status_of`]).
        needs_admin: bool,
    },
}

/// Decide what installing `id` by `method` on `os` means, refusing anything
/// the user did not consent to or that is not on offer. Pure: `has_tool`
/// says whether winget/brew are present.
pub fn plan(
    id: Dependency,
    method: InstallMethod,
    consent: bool,
    os: Os,
    has_tool: &dyn Fn(InstallTool) -> bool,
) -> Result<Plan, IpcError> {
    if !consent {
        return Err(IpcError::localized(
            IpcErrorCode::InvalidRequest,
            "install_consent_required",
            "installing a dependency needs the user's consent",
        ));
    }
    let spec = deps::spec(id, os);
    let Some(recipe) = spec.install.iter().find(|r| r.method == method) else {
        return Err(IpcError::localized(
            IpcErrorCode::InvalidRequest,
            "install_not_offered",
            format!("{method:?} is not offered for {} here", spec.name),
        ));
    };
    match recipe.tool() {
        None => Ok(Plan::Open(recipe.url().unwrap_or_default())),
        Some(tool) if has_tool(tool) => Ok(Plan::Run {
            tool,
            args: recipe.args(),
            needs_admin: recipe.needs_admin,
        }),
        Some(tool) => Err(IpcError::localized(
            IpcErrorCode::InvalidRequest,
            "install_tool_missing",
            format!("{} is not installed", tool_name(tool)),
        )
        .with_param("tool", tool_name(tool))),
    }
}

fn tool_name(tool: InstallTool) -> &'static str {
    match tool {
        InstallTool::Winget => "winget",
        InstallTool::Brew => "Homebrew",
    }
}

/// Whether one piece of installer output is worth a line on screen: lines
/// that are only a spinner (`-`, `\`) or an empty progress bar are dropped.
fn worth_showing(line: &str) -> bool {
    !line.is_empty()
        && line
            .chars()
            .any(|c| c.is_alphanumeric() && !matches!(c, '█' | '▒'))
}

/// Split installer output into lines worth showing: winget redraws progress
/// with carriage returns and spinners, so `\r` ends a line too, and lines
/// that are only a spinner or a bar are dropped. Pure.
pub fn output_lines(chunk: &str) -> Vec<String> {
    chunk
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|line| worth_showing(line))
        .map(str::to_string)
        .collect()
}

/// Cuts a byte stream into lines as the bytes arrive, ending a line at `\r`
/// as well as `\n`. winget redraws its download bar with bare carriage
/// returns and writes no newline until the step ends, so splitting only on
/// `\n` would hold minutes of progress back and release it in one burst.
/// Both bytes are ASCII, so a cut never lands inside a UTF-8 sequence.
#[derive(Debug, Default)]
pub struct LineSplitter {
    pending: Vec<u8>,
}

impl LineSplitter {
    /// Feed the next bytes; returns every line they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for &b in bytes {
            if b == b'\r' || b == b'\n' {
                self.flush_into(&mut out);
            } else {
                self.pending.push(b);
            }
        }
        out
    }

    /// The stream ended: whatever is left is the last line.
    pub fn finish(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        self.flush_into(&mut out);
        out
    }

    fn flush_into(&mut self, out: &mut Vec<String>) {
        if self.pending.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(&self.pending);
        let line = text.trim();
        if worth_showing(line) {
            out.push(line.to_string());
        }
        self.pending.clear();
    }
}

/// How [`run_streaming`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    /// The program exited on its own, with this code (`None`: the OS gave
    /// none, e.g. it was killed by a signal).
    Exited(Option<i32>),
    /// It ran past the time allowed and was stopped; `tree_stopped` says
    /// whether everything it started was confirmed gone.
    TimedOut {
        /// Whether the whole process tree was confirmed ended.
        tree_stopped: bool,
    },
    /// The user stopped it ([`cancel_dependency_install`]); `tree_stopped`
    /// says whether everything it started was confirmed gone
    /// ([`process::kill_tree_confirmed`]).
    Cancelled {
        /// Whether the whole process tree was confirmed ended.
        tree_stopped: bool,
    },
}

/// How long the output readers may keep draining once the installer itself
/// has ended. A helper the installer started (an MSI, a cask's `pkg`) can
/// inherit the pipes and hold them open for as long as it lives; the install
/// is over for us either way, so the readers are left behind rather than
/// joined forever.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Run `program args`, sending each output line to `on_line` as it is
/// written (split on `\r` and `\n`), stopping it — with everything it started
/// — at `timeout` or as soon as `cancel` is set. Returns how it ended and the
/// program's process id (on Unix also its process group's).
pub fn run_streaming(
    program: &PathBuf,
    args: &[String],
    timeout: Duration,
    cancel: &AtomicBool,
    on_line: &dyn Fn(String),
) -> std::io::Result<(RunEnd, u32)> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Own process group on Unix (so the kill takes brew's helpers too) and no
    // console window on Windows (CREATE_NO_WINDOW).
    process::isolate_process_tree(&mut cmd);
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel::<String>();
    for stream in [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        // Detached on purpose: see DRAIN_GRACE.
        std::thread::spawn(move || {
            let mut stream = stream;
            let mut splitter = LineSplitter::default();
            let mut buf = [0u8; 4096];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for line in splitter.push(&buf[..n]) {
                            if tx.send(line).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
            for line in splitter.finish() {
                let _ = tx.send(line);
            }
        });
    }
    drop(tx);
    let started = Instant::now();
    let end = loop {
        while let Ok(line) = rx.try_recv() {
            on_line(line);
        }
        if cancel.load(Ordering::SeqCst) {
            let tree_stopped = process::kill_tree_confirmed(&mut child);
            break RunEnd::Cancelled { tree_stopped };
        }
        if started.elapsed() > timeout {
            let tree_stopped = process::kill_tree_confirmed(&mut child);
            break RunEnd::TimedOut { tree_stopped };
        }
        if let Some(status) = child.try_wait()? {
            break RunEnd::Exited(status.code());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    // Drain what is still buffered, but never wait on a pipe some grandchild
    // keeps open: the readers end when every writer is gone, or are left.
    let deadline = Instant::now() + DRAIN_GRACE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) => on_line(line),
            Err(_) => break,
        }
    }
    Ok((end, pid))
}

/// Holds the one-install-at-a-time slot. Releasing it — and clearing a
/// pending cancel — happens on drop, so every way out of an install (success,
/// error, a panic in the blocking task, a dropped future) frees the slot; a
/// stuck slot would refuse every later install with `install_in_progress`
/// until the app restarted.
pub struct InstallSlot(());

impl InstallSlot {
    /// Take the slot for installing `id` by `method` (`elevated`: with
    /// administrator rights), or `None` when another install holds it.
    pub fn acquire(id: Dependency, method: InstallMethod, elevated: bool) -> Option<Self> {
        if INSTALLING.swap(true, Ordering::SeqCst) {
            return None;
        }
        CANCEL.store(false, Ordering::SeqCst);
        let mut reg = registry();
        reg.seq = reg.seq.wrapping_add(1);
        let started_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        reg.running = Some(RunningInstall {
            seq: reg.seq,
            id,
            name: deps::spec(id, Os::current()).name.to_string(),
            method,
            started_at,
            elevated,
            may_continue: false,
        });
        Some(Self(()))
    }

    /// The install ended with `outcome`. Recorded for
    /// [`dependency_install_status`]; when the installer may still be running
    /// the slot is marked so, and nothing can be stopped any more.
    pub fn finish(&self, outcome: &InstallOutcome) {
        let mut reg = registry();
        let Some(running) = reg.running.as_mut() else {
            return;
        };
        running.may_continue = outcome.status.may_continue();
        let finished = FinishedInstall {
            seq: running.seq,
            id: running.id,
            outcome: Some(outcome.clone()),
        };
        reg.last = Some(finished);
    }
}

impl Drop for InstallSlot {
    fn drop(&mut self) {
        {
            let mut reg = registry();
            if let Some(running) = reg.running.take() {
                // An install that ends without `finish` (a panic, an early
                // error) still leaves an answer for a window waiting on it.
                if reg.last.as_ref().is_none_or(|l| l.seq != running.seq) {
                    reg.last = Some(FinishedInstall {
                        seq: running.seq,
                        id: running.id,
                        outcome: None,
                    });
                }
            }
        }
        CANCEL.store(false, Ordering::SeqCst);
        INSTALLING.store(false, Ordering::SeqCst);
    }
}

/// How long the slot stays taken after an install whose installer may have
/// outlived it ([`linger`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LingerPolicy {
    /// Held at least this long, even when the probe sees nothing: an
    /// elevated installer between its UAC prompt and its first step shows
    /// no sign of life yet.
    pub min: Duration,
    /// Released after this long, whatever the probe says: a wrong "still
    /// running" must not refuse every install until Pluryprint restarts.
    pub max: Duration,
    /// How often the probe is asked.
    pub tick: Duration,
}

/// Grace period when nothing can tell whether the installer is still
/// running (a Windows installer that is not elevated and refused the kill),
/// and the least an elevated one is waited for.
pub const LINGER_UNKNOWN: Duration = Duration::from_secs(60);
/// The longest a may-continue installer keeps the slot.
pub const LINGER_MAX: Duration = Duration::from_secs(15 * 60);

/// A probe for [`linger`]: `Some(true)` while the installer runs,
/// `Some(false)` once it is gone, `None` when that cannot be told.
pub type AliveProbe = Box<dyn FnMut() -> Option<bool> + Send>;

/// The linger policy for an installer that may still be running, and the
/// probe that tells whether it is. Unix: the install's own process group
/// (`pgid`) is watched. Windows: an elevated installer is watched through
/// the Windows Installer's mutex; a non-elevated one cannot be seen, so the
/// slot is held for [`LINGER_UNKNOWN`].
pub fn linger_plan(elevated: bool, pgid: u32) -> (LingerPolicy, AliveProbe) {
    let tick = Duration::from_secs(2);
    #[cfg(unix)]
    {
        let _ = elevated;
        (
            LingerPolicy {
                min: Duration::ZERO,
                max: LINGER_MAX,
                tick,
            },
            Box::new(move || process::group_alive(pgid)),
        )
    }
    #[cfg(windows)]
    {
        let _ = pgid;
        if elevated {
            (
                LingerPolicy {
                    min: LINGER_UNKNOWN,
                    max: LINGER_MAX,
                    tick,
                },
                Box::new(|| Some(process::msi_install_running())),
            )
        } else {
            (
                LingerPolicy {
                    min: LINGER_UNKNOWN,
                    max: LINGER_UNKNOWN,
                    tick,
                },
                Box::new(|| None),
            )
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (elevated, pgid);
        (
            LingerPolicy {
                min: LINGER_UNKNOWN,
                max: LINGER_UNKNOWN,
                tick,
            },
            Box::new(|| None),
        )
    }
}

/// Keep `held` (the install slot) until `probe` says the installer is gone
/// (`Some(false)`) once `policy.min` has passed, or until `policy.max`;
/// then drop it. `elapsed` and `sleep` are the clock, injected for tests.
pub fn linger<T>(
    held: T,
    policy: LingerPolicy,
    mut probe: impl FnMut() -> Option<bool>,
    mut elapsed: impl FnMut() -> Duration,
    mut sleep: impl FnMut(Duration),
) {
    loop {
        let e = elapsed();
        if e >= policy.max {
            break;
        }
        if e >= policy.min && probe() == Some(false) {
            break;
        }
        sleep(policy.tick);
    }
    drop(held);
}

/// What [`InstallStatus`] (and exit code) an install that ran a command ended
/// in. `elevated`: the installer runs with administrator rights on Windows,
/// where a cancel or a timeout never reaches it for sure (an elevated process refuses a
/// non-elevated kill, and an MSI runs in the Windows Installer service). A
/// stop — the user's or the timeout's — is reported as done only when
/// nothing can outlive it. Pure.
pub fn status_of(end: RunEnd, found: bool, elevated: bool) -> (InstallStatus, Option<i32>) {
    match end {
        RunEnd::Exited(Some(0)) if found => (InstallStatus::Installed, Some(0)),
        RunEnd::Exited(Some(0)) => (InstallStatus::NotDetected, Some(0)),
        RunEnd::Exited(code) => (InstallStatus::Failed, code),
        RunEnd::TimedOut { tree_stopped } if tree_stopped && !elevated => {
            (InstallStatus::TimedOut, None)
        }
        RunEnd::TimedOut { .. } => (InstallStatus::TimedOutInstallerMayContinue, None),
        RunEnd::Cancelled { tree_stopped } if tree_stopped && !elevated => {
            (InstallStatus::Cancelled, None)
        }
        RunEnd::Cancelled { .. } => (InstallStatus::CancelRequestedInstallerMayContinue, None),
    }
}

/// Install the dependency `id` the way the user agreed to (`method`), after
/// the consent dialog (`consent` must be `true`). See the module docs.
#[tauri::command]
#[specta::specta]
pub async fn install_dependency(
    app: tauri::AppHandle,
    id: Dependency,
    method: InstallMethod,
    consent: bool,
) -> Result<InstallOutcome, IpcError> {
    let os = Os::current();
    let plan = plan(id, method, consent, os, &|tool| {
        deps::locate_tool(tool).is_some()
    })?;
    match plan {
        Plan::Open(url) => {
            use tauri_plugin_opener::OpenerExt;
            app.opener().open_url(&url, None::<&str>).map_err(|e| {
                IpcError::localized(
                    IpcErrorCode::Internal,
                    "open_link_failed",
                    format!("could not open {url}: {e}"),
                )
            })?;
            tracing::info!(event = "dependency_install_opened", dependency = ?id, url = %url);
            Ok(InstallOutcome {
                status: InstallStatus::Opened,
                exit_code: None,
                elevated: false,
                report: deps::report(id, os, None),
            })
        }
        Plan::Run {
            tool,
            args,
            needs_admin,
        } => {
            let elevated = needs_admin && os == Os::Windows;
            let Some(slot) = InstallSlot::acquire(id, method, elevated) else {
                return Err(IpcError::localized(
                    IpcErrorCode::InvalidRequest,
                    "install_in_progress",
                    "another dependency is being installed",
                ));
            };
            super::run_blocking(move || {
                // Moved in, so the slot is released when this task ends, by
                // whatever path — or handed to the linger thread below.
                let slot = slot;
                let program = deps::locate_tool(tool).ok_or_else(|| {
                    IpcError::localized(
                        IpcErrorCode::InvalidRequest,
                        "install_tool_missing",
                        format!("{} is not installed", tool_name(tool)),
                    )
                    .with_param("tool", tool_name(tool))
                })?;
                tracing::info!(event = "dependency_install_started", dependency = ?id, program = %program.display(), args = ?args);
                let emit = |line: String| {
                    let _ = DependencyInstallProgress { id, line }.emit(&app);
                };
                let end = run_streaming(&program, &args, INSTALL_TIMEOUT, &CANCEL, &emit)
                    .map_err(|e| {
                        IpcError::localized(
                            IpcErrorCode::Internal,
                            "install_start_failed",
                            format!("could not start {}: {e}", program.display()),
                        )
                        .with_param("tool", tool_name(tool))
                    })?;
                let (end, pid) = end;
                let report = deps::report(id, os, None);
                let (status, exit_code) = status_of(end, report.found, elevated);
                tracing::info!(event = "dependency_install_finished", dependency = ?id, ?end, ?status);
                let outcome = InstallOutcome {
                    status,
                    exit_code,
                    elevated,
                    report,
                };
                slot.finish(&outcome);
                if status.may_continue() {
                    // The installer may still be working: keep the slot so a
                    // second install cannot start beside it, without holding
                    // the answer back.
                    let (policy, probe) = linger_plan(elevated, pid);
                    std::thread::spawn(move || {
                        let started = Instant::now();
                        linger(slot, policy, probe, || started.elapsed(), std::thread::sleep);
                        tracing::info!(event = "dependency_install_slot_released", dependency = ?id, after = ?started.elapsed());
                    });
                }
                Ok(outcome)
            })
            .await
        }
    }
}

/// Stop the install that is running, if any, with every process it started
/// (FR-058: the user can always back out of an install they began). Returns
/// whether there was one to stop; the install's own call then resolves with
/// [`InstallStatus::Cancelled`], or with
/// [`InstallStatus::CancelRequestedInstallerMayContinue`] when the installer
/// could not be confirmed stopped.
#[tauri::command]
#[specta::specta]
pub fn cancel_dependency_install() -> Result<bool, IpcError> {
    if !INSTALLING.load(Ordering::SeqCst) {
        return Ok(false);
    }
    // An install that already ended and only waits for its installer to go
    // away has nothing left that Pluryprint could stop.
    if registry().running.as_ref().is_some_and(|r| r.may_continue) {
        return Ok(false);
    }
    CANCEL.store(true, Ordering::SeqCst);
    tracing::info!(event = "dependency_install_cancel_requested");
    Ok(true)
}

/// Which install holds the one-install slot, and how the last one ended
/// (FR-058). The UI asks on startup, so a reloaded window shows a running
/// install — with its Stop — instead of an Install the backend would refuse.
#[tauri::command]
#[specta::specta]
pub fn dependency_install_status() -> Result<DependencyInstallState, IpcError> {
    let reg = registry();
    Ok(DependencyInstallState {
        running: reg.running.clone(),
        last: reg.last.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot and its registry are process-wide: tests that take the slot
    /// run one at a time.
    static SLOT_TESTS: Mutex<()> = Mutex::new(());

    fn slot_lock() -> MutexGuard<'static, ()> {
        SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn outcome(status: InstallStatus, elevated: bool) -> InstallOutcome {
        InstallOutcome {
            status,
            exit_code: None,
            elevated,
            report: deps::report(Dependency::LibreOffice, Os::current(), None),
        }
    }

    #[test]
    fn fr_058_install_requires_consent() {
        let err = plan(
            Dependency::LibreOffice,
            InstallMethod::Winget,
            false,
            Os::Windows,
            &|_| true,
        )
        .unwrap_err();
        assert_eq!(err.code(), IpcErrorCode::InvalidRequest);
        assert!(
            format!("{err:?}").contains("install_consent_required"),
            "{err:?}"
        );
        // Not even a web page opens without consent.
        assert!(plan(
            Dependency::LibreOffice,
            InstallMethod::Web,
            false,
            Os::Windows,
            &|_| true
        )
        .is_err());
    }

    #[test]
    fn fr_058_winget_absent_refuses_and_web_still_works() {
        let err = plan(
            Dependency::LibreOffice,
            InstallMethod::Winget,
            true,
            Os::Windows,
            &|_| false,
        )
        .unwrap_err();
        assert!(
            format!("{err:?}").contains("install_tool_missing"),
            "{err:?}"
        );
        assert_eq!(
            plan(
                Dependency::LibreOffice,
                InstallMethod::Web,
                true,
                Os::Windows,
                &|_| false
            )
            .unwrap(),
            Plan::Open("https://www.libreoffice.org/download/download-libreoffice/".into())
        );
    }

    #[test]
    fn fr_058_only_offered_methods_run() {
        // HEVC is paid: winget cannot buy it, so there is no winget recipe.
        let err = plan(
            Dependency::HevcCodec,
            InstallMethod::WingetStore,
            true,
            Os::Windows,
            &|_| true,
        )
        .unwrap_err();
        assert!(
            format!("{err:?}").contains("install_not_offered"),
            "{err:?}"
        );
        // Nothing is installable for a codec on macOS.
        assert!(plan(
            Dependency::HeifCodec,
            InstallMethod::Web,
            true,
            Os::MacOs,
            &|_| true
        )
        .is_err());
        match plan(
            Dependency::LibreOffice,
            InstallMethod::BrewCask,
            true,
            Os::MacOs,
            &|t| t == InstallTool::Brew,
        )
        .unwrap()
        {
            Plan::Run {
                tool,
                args,
                needs_admin,
            } => {
                assert!(!needs_admin);
                assert_eq!(tool, InstallTool::Brew);
                assert_eq!(args, ["install", "--cask", "libreoffice"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn fr_058_installer_output_is_split_into_readable_lines() {
        assert_eq!(
            output_lines("Found LibreOffice [TheDocumentFoundation.LibreOffice]\r\n  -\r  \\\r  ██████▒▒▒▒  20 MB / 350 MB\rSuccessfully installed\n"),
            [
                "Found LibreOffice [TheDocumentFoundation.LibreOffice]",
                "██████▒▒▒▒  20 MB / 350 MB",
                "Successfully installed"
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn fr_058_run_streaming_reports_lines_and_the_exit_code() {
        let never = AtomicBool::new(false);
        let lines = std::sync::Mutex::new(Vec::new());
        let end = run_streaming(
            &PathBuf::from("/bin/sh"),
            &["-c".into(), "echo one; echo two >&2; exit 3".into()],
            Duration::from_secs(10),
            &never,
            &|l| lines.lock().unwrap().push(l),
        )
        .unwrap()
        .0;
        assert_eq!(end, RunEnd::Exited(Some(3)));
        let mut got = lines.into_inner().unwrap();
        got.sort();
        assert_eq!(got, ["one", "two"]);

        let started = Instant::now();
        let end = run_streaming(
            &PathBuf::from("/bin/sh"),
            &["-c".into(), "sleep 20".into()],
            Duration::from_millis(300),
            &never,
            &|_| {},
        )
        .unwrap()
        .0;
        assert_eq!(
            end,
            RunEnd::TimedOut { tree_stopped: true },
            "stopped at the timeout, with its whole tree"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn fr_058_timeout_and_cancel_have_their_own_status_not_an_english_line() {
        assert_eq!(
            status_of(RunEnd::TimedOut { tree_stopped: true }, false, false),
            (InstallStatus::TimedOut, None)
        );
        assert_eq!(
            status_of(RunEnd::Cancelled { tree_stopped: true }, false, false),
            (InstallStatus::Cancelled, None)
        );
        assert_eq!(
            status_of(RunEnd::Exited(Some(0)), true, false),
            (InstallStatus::Installed, Some(0))
        );
        assert_eq!(
            status_of(RunEnd::Exited(Some(0)), false, false),
            (InstallStatus::NotDetected, Some(0))
        );
        assert_eq!(
            status_of(RunEnd::Exited(None), false, false),
            (InstallStatus::Failed, None)
        );
    }

    #[test]
    fn fr_058_winget_progress_streams_on_carriage_returns() {
        let mut s = LineSplitter::default();
        // A bar redrawn with bare \r: each redraw is a line as soon as the
        // next \r arrives, without waiting for a newline.
        assert_eq!(
            s.push(b"Downloading 10 MB / 350 MB\r"),
            ["Downloading 10 MB / 350 MB"]
        );
        assert_eq!(s.push(b"  -\r  \\\r"), Vec::<String>::new());
        assert_eq!(s.push(b"Downloading 20 MB"), Vec::<String>::new());
        assert_eq!(s.push(b" / 350 MB\r"), ["Downloading 20 MB / 350 MB"]);
        assert_eq!(
            s.push("Instalado correctamente".as_bytes()),
            Vec::<String>::new()
        );
        assert_eq!(s.finish(), ["Instalado correctamente"]);
    }

    #[cfg(unix)]
    #[test]
    fn fr_058_run_streaming_emits_a_cr_line_before_the_program_ends() {
        let never = AtomicBool::new(false);
        let (tx, rx) = mpsc::channel::<(String, Instant)>();
        let started = Instant::now();
        let end = run_streaming(
            &PathBuf::from("/bin/sh"),
            &[
                "-c".into(),
                "printf 'progress 1\\r'; sleep 1; printf 'done\\n'".into(),
            ],
            Duration::from_secs(10),
            &never,
            &move |l| {
                let _ = tx.send((l, Instant::now()));
            },
        )
        .unwrap()
        .0;
        assert_eq!(end, RunEnd::Exited(Some(0)));
        let got: Vec<_> = rx.try_iter().collect();
        assert_eq!(got[0].0, "progress 1");
        assert!(
            got[0].1.duration_since(started) < Duration::from_millis(900),
            "the \\r line arrived only after the program ended"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fr_058_a_grandchild_holding_the_pipe_does_not_hang_the_install() {
        let never = AtomicBool::new(false);
        let started = Instant::now();
        // The background sleep inherits stdout and keeps the pipe open after
        // the shell exits — like an MSI the installer started.
        let end = run_streaming(
            &PathBuf::from("/bin/sh"),
            &["-c".into(), "sleep 8 & echo started; exit 0".into()],
            Duration::from_secs(30),
            &never,
            &|_| {},
        )
        .unwrap()
        .0;
        assert_eq!(end, RunEnd::Exited(Some(0)));
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "waited on the grandchild's pipe: {:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn fr_058_cancel_kills_the_install_tree() {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let end = run_streaming(
            &PathBuf::from("/bin/sh"),
            &["-c".into(), "sleep 20 & sleep 20".into()],
            Duration::from_secs(60),
            &cancel,
            &|_| {},
        )
        .unwrap()
        .0;
        assert_eq!(
            end,
            RunEnd::Cancelled { tree_stopped: true },
            "a tree we own is confirmed gone"
        );
        assert!(started.elapsed() < Duration::from_secs(6));
    }

    /// Round-5 finding: cancelling an elevated winget install said
    /// "stopped" although the installer kept running. A cancel is reported
    /// as done only when the whole tree is confirmed gone and nothing ran
    /// elevated; otherwise the user is told the installer may continue.
    #[test]
    fn fr_058_a_cancel_that_cannot_reach_the_installer_says_so() {
        let may_continue = (InstallStatus::CancelRequestedInstallerMayContinue, None);
        assert_eq!(
            status_of(
                RunEnd::Cancelled {
                    tree_stopped: false
                },
                false,
                false
            ),
            may_continue,
            "part of the tree refused the kill"
        );
        assert_eq!(
            status_of(RunEnd::Cancelled { tree_stopped: true }, false, true),
            may_continue,
            "an elevated MSI runs outside the tree"
        );
        // Even when the dependency shows up meanwhile, a cancel is a cancel.
        assert_eq!(
            status_of(
                RunEnd::Cancelled {
                    tree_stopped: false
                },
                true,
                true
            ),
            may_continue
        );
        // LibreOffice through winget needs admin rights: the plan says so.
        match plan(
            Dependency::LibreOffice,
            InstallMethod::Winget,
            true,
            Os::Windows,
            &|_| true,
        )
        .unwrap()
        {
            Plan::Run { needs_admin, .. } => assert!(needs_admin),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            serde_json::to_value(InstallStatus::CancelRequestedInstallerMayContinue).unwrap(),
            "cancel-requested-installer-may-continue"
        );
    }

    #[test]
    fn fr_058_install_slot_is_released_on_every_exit_path() {
        let _serial = slot_lock();
        let take = || InstallSlot::acquire(Dependency::LibreOffice, InstallMethod::Winget, false);
        let slot = take().expect("free at first");
        assert!(take().is_none(), "one install at a time");
        assert!(
            cancel_dependency_install().unwrap(),
            "a running install can be stopped"
        );
        drop(slot);
        assert!(
            !cancel_dependency_install().unwrap(),
            "nothing left to stop"
        );
        // A panic inside the install still frees the slot.
        let caught = std::panic::catch_unwind(|| {
            let _slot = take().expect("free again");
            panic!("installer task blew up");
        });
        assert!(caught.is_err());
        let again = take();
        assert!(again.is_some(), "the slot leaked after a panic");
        assert!(
            !CANCEL.load(Ordering::SeqCst),
            "a fresh install starts uncancelled"
        );
    }

    /// Round-6 finding: a timeout said "was stopped" even when an elevated
    /// installer, or part of the tree that refused the kill, kept running.
    #[test]
    fn fr_058_a_timeout_the_installer_may_outlive_says_so() {
        let may_continue = (InstallStatus::TimedOutInstallerMayContinue, None);
        assert_eq!(
            status_of(
                RunEnd::TimedOut {
                    tree_stopped: false
                },
                false,
                false
            ),
            may_continue,
            "part of the tree refused the kill"
        );
        assert_eq!(
            status_of(RunEnd::TimedOut { tree_stopped: true }, false, true),
            may_continue,
            "an elevated MSI runs outside the tree"
        );
        assert!(InstallStatus::TimedOutInstallerMayContinue.may_continue());
        assert!(InstallStatus::CancelRequestedInstallerMayContinue.may_continue());
        assert!(!InstallStatus::TimedOut.may_continue());
        assert!(!InstallStatus::Cancelled.may_continue());
        assert_eq!(
            serde_json::to_value(InstallStatus::TimedOutInstallerMayContinue).unwrap(),
            "timed-out-installer-may-continue"
        );
    }

    /// A fake clock for [`linger`]: every sleep advances it.
    struct Clock(std::cell::Cell<Duration>);

    impl Clock {
        fn new() -> Self {
            Self(std::cell::Cell::new(Duration::ZERO))
        }
        fn now(&self) -> Duration {
            self.0.get()
        }
        fn sleep(&self, d: Duration) {
            self.0.set(self.0.get() + d);
        }
    }

    /// Drops into a flag, standing in for the install slot.
    struct Held<'a>(&'a std::cell::Cell<bool>);

    impl Drop for Held<'_> {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    const POLICY: LingerPolicy = LingerPolicy {
        min: Duration::from_secs(10),
        max: Duration::from_secs(100),
        tick: Duration::from_secs(2),
    };

    /// Round-6 finding: the slot was freed while a may-continue installer
    /// could still run, so a second install could start beside it.
    #[test]
    fn fr_058_a_may_continue_installer_keeps_the_slot_until_it_is_gone() {
        let clock = Clock::new();
        let released = std::cell::Cell::new(false);
        let mut alive_until = Duration::from_secs(30);
        linger(
            Held(&released),
            POLICY,
            || Some(clock.now() < alive_until),
            || clock.now(),
            |d| {
                assert!(!released.get(), "released while the installer ran");
                clock.sleep(d);
            },
        );
        assert!(released.get());
        assert_eq!(clock.now(), Duration::from_secs(30), "released once gone");

        // Gone at once: still held for the minimum (an elevated installer
        // may not show yet).
        let clock = Clock::new();
        alive_until = Duration::ZERO;
        linger(
            (),
            POLICY,
            || Some(clock.now() < alive_until),
            || clock.now(),
            |d| clock.sleep(d),
        );
        assert_eq!(clock.now(), POLICY.min);

        // Never gone, or nothing can tell: released at the maximum.
        for probe in [Some(true), None] {
            let clock = Clock::new();
            linger((), POLICY, || probe, || clock.now(), |d| clock.sleep(d));
            assert_eq!(clock.now(), POLICY.max, "{probe:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn fr_058_linger_watches_the_install_process_group() {
        // An installer in its own group, as run_streaming starts it, with a
        // helper that outlives its parent shell.
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "sleep 30 & exit 0"]);
        process::isolate_process_tree(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let pgid = child.id();
        child.wait().unwrap();
        let (policy, mut alive) = linger_plan(false, pgid);
        assert_eq!(policy.min, Duration::ZERO);
        assert_eq!(policy.max, LINGER_MAX);
        assert_eq!(alive(), Some(true), "the helper still runs");
        // `--` first: procps-ng 3.3.17 (Ubuntu 22.04) reads `-<pgid>` right
        // after a signal option as a second option and kills the caller's
        // own group, the test runner included.
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{pgid}")])
            .status();
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive() != Some(false) {
            assert!(Instant::now() < deadline, "the group never went away");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Round-6 finding: after a reload the window lost the running install;
    /// the backend now says which install holds the slot and how the last
    /// one ended.
    #[test]
    fn fr_058_install_status_reports_the_running_install() {
        let _serial = slot_lock();
        let slot = InstallSlot::acquire(Dependency::LibreOffice, InstallMethod::Winget, true)
            .expect("free");
        let state = dependency_install_status().unwrap();
        let running = state.running.expect("running");
        assert_eq!(running.id, Dependency::LibreOffice);
        assert_eq!(running.method, InstallMethod::Winget);
        assert!(running.elevated);
        assert!(!running.may_continue);
        assert!(
            time::OffsetDateTime::parse(
                &running.started_at,
                &time::format_description::well_known::Rfc3339
            )
            .is_ok(),
            "{}",
            running.started_at
        );

        // It ends, but the installer may continue: the slot stays taken,
        // the end is readable, and nothing is left to stop.
        let ended = outcome(InstallStatus::CancelRequestedInstallerMayContinue, true);
        slot.finish(&ended);
        let state = dependency_install_status().unwrap();
        assert!(state.running.as_ref().expect("still held").may_continue);
        let last = state.last.expect("last");
        assert_eq!(last.seq, running.seq);
        assert_eq!(last.outcome, Some(ended));
        assert!(
            InstallSlot::acquire(Dependency::HeifCodec, InstallMethod::WingetStore, false)
                .is_none(),
            "no second install beside a may-continue installer"
        );
        assert!(!cancel_dependency_install().unwrap(), "nothing to stop");

        drop(slot);
        let state = dependency_install_status().unwrap();
        assert!(state.running.is_none());
        assert_eq!(state.last.unwrap().seq, running.seq);

        // An install that ends without an outcome still answers.
        let slot = InstallSlot::acquire(Dependency::HeifCodec, InstallMethod::WingetStore, false)
            .expect("free again");
        drop(slot);
        let last = dependency_install_status().unwrap().last.unwrap();
        assert_eq!(last.id, Dependency::HeifCodec);
        assert_eq!(last.seq, running.seq + 1);
        assert!(last.outcome.is_none());
    }
}
