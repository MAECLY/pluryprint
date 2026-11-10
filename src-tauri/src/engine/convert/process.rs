//! Bounded execution of an external converter (LibreOffice, `sips`, `textutil`,
//! the render and SVG workers).
//!
//! Every converter that shells out goes through [`run_bounded`], which is where
//! the three promises of `docs/03-requirements/supported-formats.md` live:
//!
//! - **No shell.** The program and its arguments are a [`Command`] argument
//!   vector; nothing is ever interpolated into a shell string, so a file called
//!   `$(rm -rf ~).docx` is just an odd file name.
//! - **Bounded.** The child is polled against a deadline *and* the job's cancel
//!   flag. The dispatcher's `conversion_timeout` sets that flag, so a hung
//!   `soffice` is killed rather than left to hold a conversion slot forever.
//! - **Killed for real.** Office suites and browsers fork helpers
//!   (`soffice.bin`, renderer processes). On Unix the child leads its own
//!   process group and the whole group is killed; on Windows `taskkill /T`
//!   takes the tree. `Child::kill` is always issued as well, as the floor.
//!
//! stderr goes to a file in the work directory rather than a pipe: a chatty
//! child can never fill a pipe buffer and deadlock against our polling loop,
//! and the tail of it becomes the user-visible failure detail.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

use crate::engine::{EngineError, Failure};

/// How often the cancel flag and the deadline are checked while a child runs.
const POLL: Duration = Duration::from_millis(100);

/// At most this many bytes of the child's stderr are quoted in an error.
const STDERR_TAIL: usize = 600;

/// Why [`run_bounded`] stopped a child early.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stopped {
    /// The job's cancel flag was raised (user cancel, pause, or the
    /// dispatcher's conversion timeout).
    Cancelled,
    /// The converter's own deadline passed.
    TimedOut,
    /// The OS could not tell us whether the child was still running, so it
    /// was killed rather than waited on blindly. Carries the OS error text:
    /// this is not a timeout and must not be reported as one.
    WaitFailed(String),
}

/// Run `cmd` to completion, killing it when `cancel` is raised or `timeout`
/// elapses.
///
/// `label` names the program in error messages ("LibreOffice"). `stderr_log`
/// is where the child's stderr is captured; it should live inside the job's
/// work directory so it disappears with it.
///
/// Returns `Ok(())` only for a zero exit status. A non-zero exit is
/// [`EngineError::ConversionFailed`] carrying the tail of stderr; a spawn
/// failure of a program we already located is the same (a broken install, not
/// a missing one); a cancel is [`EngineError::Cancelled`]; a timeout is
/// [`EngineError::ConversionTimedOut`] naming the limit (terminal: the queue
/// does not retry a converter that already ran out of time); a failed wait is
/// `ConversionFailed` carrying the OS error.
pub fn run_bounded(
    cmd: Command,
    label: &str,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    stderr_log: &Path,
) -> Result<(), EngineError> {
    run_until(cmd, label, cancel, timeout, stderr_log, None)
}

/// [`run_bounded`], plus a `finished` predicate checked on every poll: once it
/// answers `true` the child (and its helpers) are killed and the run counts as
/// a success.
///
/// It was written for headless Chromium, which on some versions wrote its
/// `--print-to-pdf` file and then never exited; the browser fallback is gone,
/// and the render worker exits by itself, so every caller now passes `None`.
/// Kept for a converter whose output is complete before it exits.
pub fn run_until(
    mut cmd: Command,
    label: &str,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    stderr_log: &Path,
    finished: Option<&dyn Fn() -> bool>,
) -> Result<(), EngineError> {
    let stderr = std::fs::File::create(stderr_log).map_err(|e| {
        EngineError::ConversionFailed(Failure::new(
            "workdir_failed",
            format!("cannot create a log for {label}: {e}"),
        ))
    })?;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr));
    isolate_process_tree(&mut cmd);

    let mut child = cmd.spawn().map_err(|e| spawn_error(label, &e))?;

    match wait_bounded(&mut child, cancel, timeout, finished) {
        Ok(Exit::Finished) => Ok(()),
        Ok(Exit::Status(status)) if status.success() => Ok(()),
        Ok(Exit::Status(status)) => Err(EngineError::ConversionFailed(
            Failure::new(
                "converter_failed",
                format!("{label} failed ({status}){}", stderr_suffix(stderr_log)),
            )
            .param("tool", label),
        )),
        Err(stopped) => Err(stopped_error(stopped, label, timeout)),
    }
}

/// The dependency id (`common:deps.<id>`) of a converter known by its
/// [`run_bounded`] label, so a converter that vanished between being located
/// and being started pauses its job with the same FR-058 remediation the
/// resolver would have given. Pure.
pub fn dependency_id_of(label: &str) -> Option<&'static str> {
    Some(match label {
        "LibreOffice" => "libre-office",
        "Pandoc" => "pandoc",
        "sips" => "sips",
        _ => return None,
    })
}

/// The error a failed spawn becomes: a program that is not there is a
/// missing dependency (FR-058); one that is there but will not start is a
/// conversion failure naming it. Pure.
pub fn spawn_error(label: &str, e: &std::io::Error) -> EngineError {
    let detail = format!("{label} could not be started: {e}");
    if e.kind() == std::io::ErrorKind::NotFound {
        EngineError::DependencyMissing(match dependency_id_of(label) {
            Some(id) => Failure::new(id, detail),
            None => detail.into(),
        })
    } else {
        EngineError::ConversionFailed(
            Failure::new("converter_start_failed", detail).param("tool", label),
        )
    }
}

/// The error a converter that ran past `limit` becomes: the terminal
/// [`EngineError::ConversionTimedOut`], naming the tool and the limit. Shared
/// with the in-process decoders (`heif::run_abandonable`). Pure.
pub fn timed_out_error(label: &str, limit: Option<Duration>) -> EngineError {
    let secs = limit.map(|t| t.as_secs()).unwrap_or(0);
    EngineError::ConversionTimedOut(
        Failure::new(
            "tool_timed_out",
            format!("{label} did not finish within {secs}s and was stopped"),
        )
        .param("tool", label)
        .param("secs", secs),
    )
}

/// The error a child stopped early becomes. Pure.
pub fn stopped_error(stopped: Stopped, label: &str, timeout: Option<Duration>) -> EngineError {
    match stopped {
        Stopped::Cancelled => EngineError::Cancelled,
        Stopped::TimedOut => timed_out_error(label, timeout),
        Stopped::WaitFailed(e) => EngineError::ConversionFailed(
            Failure::new(
                "converter_crashed",
                format!("could not wait for {label} to finish ({e}); it was stopped"),
            )
            .param("tool", label),
        ),
    }
}

/// How a child that was not stopped early ended.
#[derive(Debug)]
pub enum Exit {
    /// It exited on its own with this status.
    Status(std::process::ExitStatus),
    /// The `finished` predicate fired; the child was killed after that.
    Finished,
}

/// Poll `child` until it exits (or `finished` says its work is done), or kill
/// it (tree and all) when `cancel` is set or `timeout` elapses. Separate from
/// [`run_bounded`] so the kill policy is testable with any child.
pub fn wait_bounded(
    child: &mut Child,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    finished: Option<&dyn Fn() -> bool>,
) -> Result<Exit, Stopped> {
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            kill_tree(child);
            return Err(Stopped::Cancelled);
        }
        if finished.is_some_and(|done| done()) {
            kill_tree(child);
            return Ok(Exit::Finished);
        }
        if let Some(limit) = timeout {
            if started.elapsed() >= limit {
                kill_tree(child);
                return Err(Stopped::TimedOut);
            }
        }
        match child.wait_timeout(POLL) {
            Ok(Some(status)) => return Ok(Exit::Status(status)),
            Ok(None) => continue,
            // The OS refused to tell us; end the child rather than wait
            // blindly, and say why — this is not a timeout.
            Err(e) => {
                kill_tree(child);
                return Err(Stopped::WaitFailed(e.to_string()));
            }
        }
    }
}

/// Put the child in its own process group (Unix) so [`kill_tree`] can take its
/// helpers with it. Windows needs nothing up front: `taskkill /T` walks the
/// tree by parent pid. The Windows console window is suppressed so a
/// conversion does not flash a terminal in the user's face.
pub(crate) fn isolate_process_tree(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Kill `child` and everything it spawned, then reap it. Best effort: each
/// step ignores failures because the child may already be gone.
pub(crate) fn kill_tree(child: &mut Child) {
    let pid = child.id();
    #[cfg(unix)]
    {
        // `kill -KILL -- -<pgid>` signals the whole group the child leads.
        // The `--` is not optional: procps-ng's `kill` (3.3.17, Ubuntu 22.04)
        // reads a bare `-<pgid>` after the signal as a second signal option
        // and then signals the *caller's* group, killing this process.
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(any(unix, windows)))]
    let _ = pid;
    let _ = child.kill();
    let _ = child.wait();
}

/// [`kill_tree`], then whether the whole tree is **confirmed** gone. `false`
/// when part of it could not be ended: a helper running elevated (`Access
/// denied` to `taskkill`, `Operation not permitted` to `kill`) or one still
/// alive a second later. The caller must then not claim the work stopped.
/// Work handed to a service outside the tree (the Windows Installer service
/// runs an MSI as SYSTEM) is invisible here; callers that start such work
/// know it and say so themselves.
pub(crate) fn kill_tree_confirmed(child: &mut Child) -> bool {
    let pid = child.id();
    #[cfg(unix)]
    {
        // `--` as in [`kill_tree`]: without it procps-ng signals our group.
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
        let _ = child.wait();
        // The group is gone once signal 0 finds nobody (helpers killed a
        // moment ago may still be zombies waiting to be reaped).
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match group_alive(pid) {
                Some(false) => return true,
                None => return false,
                Some(true) => {}
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let killed = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
        let _ = child.wait();
        matches!(killed, Ok(s) if s.success())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        let _ = child.kill();
        let _ = child.wait();
        false
    }
}

/// Whether anything is left of the process group `pgid` leads (the tree
/// [`isolate_process_tree`] gave its own group): `Some(true)` while a member
/// lives — including one that refuses our signals (`Operation not
/// permitted`, a helper running as root) — `Some(false)` once none is left,
/// `None` when it cannot be told (`kill` would not run).
#[cfg(unix)]
pub(crate) fn group_alive(pgid: u32) -> Option<bool> {
    // `--` as in [`kill_tree`]: procps-ng otherwise probes our own group,
    // which is always alive, and every tree would look like it survived.
    let probe = Command::new("kill")
        .args(["-0", "--", &format!("-{pgid}")])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    if probe.status.success() {
        return Some(true);
    }
    let err = String::from_utf8_lossy(&probe.stderr).to_ascii_lowercase();
    Some(err.contains("not permitted"))
}

/// Whether the Windows Installer is executing an installation right now: it
/// holds the `Global\_MSIExecute` mutex for as long as it does, whoever
/// started it. An elevated MSI runs in the Windows Installer service, out of
/// reach of the process tree, so this is how its end can be seen. A mutex we
/// may not open (access denied) still exists.
#[cfg(windows)]
pub(crate) fn msi_install_running() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED};
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    // SAFETY: OpenMutexW takes a static, NUL-terminated wide string and
    // returns an owned handle, closed right away.
    match unsafe {
        OpenMutexW(
            SYNCHRONIZATION_SYNCHRONIZE,
            false,
            w!("Global\\_MSIExecute"),
        )
    } {
        Ok(handle) => {
            // SAFETY: the handle was just opened and is closed once.
            let _ = unsafe { CloseHandle(handle) };
            true
        }
        Err(e) => e.code() == ERROR_ACCESS_DENIED.to_hresult(),
    }
}

/// `": <last bytes of stderr>"`, or nothing when the log is empty/unreadable.
fn stderr_suffix(log: &Path) -> String {
    let tail = log_tail(log, STDERR_TAIL);
    if tail.is_empty() {
        String::new()
    } else {
        format!(": {tail}")
    }
}

/// Lines converters print on every run that say nothing about *this* failure:
/// Fontconfig's cache warnings (six of them on a stock LibreOffice) and GTK /
/// GLib / accessibility-bus chatter. Quoting them would bury the one line the
/// user needs ("Error: source file could not be loaded").
fn is_noise(line: &str) -> bool {
    let line = line.trim();
    line.is_empty()
        || line.starts_with("Fontconfig warning")
        || line.starts_with("Fontconfig error")
        || [
            "Gtk-WARNING",
            "Gdk-WARNING",
            "GLib-GIO-WARNING",
            "dbind-WARNING",
        ]
        .iter()
        .any(|tag| line.contains(tag))
}

/// The last `max` bytes of a converter's log with the known noise removed
/// ([`is_noise`]), lines joined by a space. Empty when the log is missing or
/// holds nothing but noise.
pub(crate) fn log_tail(log: &Path, max: usize) -> String {
    let Ok(bytes) = std::fs::read(log) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let text = text
        .lines()
        .filter(|l| !is_noise(l))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");
    if text.len() > max {
        // Cut on a char boundary at or after the byte offset.
        let mut start = text.len() - max;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text[start..].to_string()
    } else {
        text
    }
}

/// Why [`run_capped`] failed.
#[derive(Debug)]
pub enum CappedError {
    /// The child needed more memory than its cap: it was killed by the
    /// watchdog, an allocation was refused under the Job Object or
    /// `RLIMIT_DATA`, or the Job Object reported the limit hit.
    OverMemory,
    /// Any other failure, as [`run_bounded`] reports it.
    Engine(EngineError),
}

/// The environment variable that tells a capped child its memory cap, in
/// bytes, so it can apply it from its own side too ([`apply_memory_cap`]).
pub const MEMORY_CAP_ENV: &str = "PLURYPRINT_WORKER_MEMORY_CAP";

/// How often a capped child's memory (and the cancel flag and deadline) is
/// checked: a bomb can allocate fast, so more often than [`POLL`].
const MEMORY_POLL: Duration = Duration::from_millis(20);

/// What [`run_capped`]'s memory cap covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapScope {
    /// The child alone, which may not start helpers; the cap is per process
    /// (the SVG worker).
    Process,
    /// The child and every helper it starts, together (the render worker and
    /// the web view's own processes; ADR-0014, round 7).
    Tree,
}

/// [`run_bounded`] for a child that must also stay under `memory_cap` bytes
/// (security note 8: the hidden SVG and render workers), over `scope`.
///
/// - **Windows:** the child is assigned to a Job Object with
///   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (the child and its helpers die with
///   the job handle, even if this process crashes). For
///   [`CapScope::Process`] the job also has `JOB_OBJECT_LIMIT_PROCESS_MEMORY`
///   (allocations past the cap fail, and Rust aborts on a failed allocation)
///   and one active process (it cannot start helpers); for [`CapScope::Tree`]
///   it has `JOB_OBJECT_LIMIT_JOB_MEMORY` instead, the cap on everything in
///   the job together, helpers included. The job's completion port says
///   whether the limit was hit. The child waits until it is in the job before
///   it reads its input ([`apply_memory_cap`]), so there is no window in
///   which it runs uncapped.
/// - **Unix:** the child sets `RLIMIT_DATA` itself before it reads its input
///   ([`apply_memory_cap`]); macOS does not enforce that (nor `RLIMIT_AS`)
///   for `mmap`, so this process also samples the child's memory every
///   [`MEMORY_POLL`] (macOS: its physical footprint, compressed pages
///   included; Linux: its resident set) and kills it past the cap. For
///   [`CapScope::Tree`] it samples the child's whole process group
///   ([`group_memory_in_use`]). On macOS, WebKit's own web-content and
///   networking processes are XPC services that launchd starts, outside that
///   group: they are not covered. On Linux a helper is covered for as long
///   as it stays in the group; one that starts its own session or group
///   (`setsid`) leaves it, and the cap with it.
///
/// A child that ran out of memory is [`CappedError::OverMemory`]; a cancel,
/// a timeout or any other failure is what [`run_bounded`] would return.
pub fn run_capped(
    mut cmd: Command,
    label: &str,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    stderr_log: &Path,
    memory_cap: u64,
    scope: CapScope,
) -> Result<(), CappedError> {
    let engine = CappedError::Engine;
    let stderr = std::fs::File::create(stderr_log).map_err(|e| {
        engine(EngineError::ConversionFailed(Failure::new(
            "workdir_failed",
            format!("cannot create a log for {label}: {e}"),
        )))
    })?;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .env(MEMORY_CAP_ENV, memory_cap.to_string());
    isolate_process_tree(&mut cmd);
    let cannot_cap = |e: String| {
        engine(EngineError::ConversionFailed(
            Failure::new(
                "converter_start_failed",
                format!("{label} could not be given its memory limit: {e}"),
            )
            .param("tool", label),
        ))
    };
    #[cfg(windows)]
    let job = job::MemoryJob::new(memory_cap, scope).map_err(cannot_cap)?;
    let mut child = cmd.spawn().map_err(|e| engine(spawn_error(label, &e)))?;
    #[cfg(windows)]
    if let Err(e) = job.assign(&child) {
        kill_tree(&mut child);
        return Err(cannot_cap(e));
    }
    #[cfg(not(windows))]
    let _ = (&cannot_cap, scope);

    let started = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            kill_tree(&mut child);
            return Err(engine(EngineError::Cancelled));
        }
        if timeout.is_some_and(|limit| started.elapsed() >= limit) {
            kill_tree(&mut child);
            return Err(engine(timed_out_error(label, timeout)));
        }
        #[cfg(unix)]
        let used = match scope {
            CapScope::Process => memory_in_use(child.id()),
            CapScope::Tree => group_memory_in_use(child.id()),
        };
        #[cfg(unix)]
        if used.is_some_and(|used| used > memory_cap) {
            kill_tree(&mut child);
            return Err(CappedError::OverMemory);
        }
        match child.wait_timeout(MEMORY_POLL) {
            Ok(Some(status)) => break status,
            Ok(None) => continue,
            Err(e) => {
                kill_tree(&mut child);
                return Err(engine(stopped_error(
                    Stopped::WaitFailed(e.to_string()),
                    label,
                    timeout,
                )));
            }
        }
    };
    if status.success() {
        return Ok(());
    }
    #[cfg(windows)]
    if job.hit_memory_limit() {
        return Err(CappedError::OverMemory);
    }
    if allocation_failed(stderr_log) {
        return Err(CappedError::OverMemory);
    }
    Err(engine(EngineError::ConversionFailed(
        Failure::new(
            "converter_failed",
            format!("{label} failed ({status}){}", stderr_suffix(stderr_log)),
        )
        .param("tool", label),
    )))
}

/// Whether the child's log says an allocation failed: Rust's default
/// allocation-error handler prints `memory allocation of N bytes failed`
/// before it aborts, which is what a refused allocation under the Job Object
/// or `RLIMIT_DATA` ends in.
fn allocation_failed(log: &Path) -> bool {
    std::fs::read(log).is_ok_and(|bytes| {
        let text = String::from_utf8_lossy(&bytes);
        text.lines()
            .any(|l| l.contains("memory allocation of") && l.contains("failed"))
    })
}

/// The memory process `pid` is using, in bytes: on macOS its physical
/// footprint (what Activity Monitor shows, compressed pages included), on
/// Linux its resident set. `None` when it cannot be read (the process is
/// gone).
#[cfg(unix)]
pub fn memory_in_use(pid: u32) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let pid = libc::c_int::try_from(pid).ok()?;
        // SAFETY: `info` is a plain C struct the call fills; the buffer it is
        // given is that struct, of the flavour it asks for.
        let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::proc_pid_rusage(
                pid,
                libc::RUSAGE_INFO_V2,
                (&mut info as *mut libc::rusage_info_v2).cast::<libc::rusage_info_t>(),
            )
        };
        (rc == 0).then_some(info.ri_phys_footprint.max(info.ri_resident_size))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        Some(pages.saturating_mul(u64::try_from(page).ok()?))
    }
}

/// The memory the process group `pgid` (a child [`isolate_process_tree`]
/// made a group leader) is using together, as [`memory_in_use`] counts it:
/// every member, not only the leader, so a helper the child starts cannot
/// grow past the cap unseen. `None` when the group is gone.
#[cfg(unix)]
pub fn group_memory_in_use(pgid: u32) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let group = libc::pid_t::try_from(pgid).ok()?;
        let mut pids = vec![0 as libc::pid_t; 256];
        // SAFETY: the buffer is `pids`, of the byte size passed; the call
        // writes at most that many pids and returns how many it wrote.
        let n = unsafe {
            libc::proc_listpgrppids(
                group,
                pids.as_mut_ptr().cast(),
                libc::c_int::try_from(pids.len() * std::mem::size_of::<libc::pid_t>()).ok()?,
            )
        };
        let n = usize::try_from(n).ok().filter(|n| *n > 0)?;
        let total = pids
            .iter()
            .take(n.min(pids.len()))
            .filter_map(|pid| memory_in_use(u32::try_from(*pid).ok()?))
            .sum::<u64>();
        // The leader counts even when the listing missed it.
        Some(total.max(memory_in_use(pgid).unwrap_or(0)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Linux has no call that lists a group's members, so every process
        // in /proc is asked for its group. A process that exits between the
        // listing and the read is simply skipped.
        let mut members = 0usize;
        let mut total = 0u64;
        for entry in std::fs::read_dir("/proc").ok()?.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            if proc_stat_pgrp(&stat) != Some(pgid) {
                continue;
            }
            if let Some(used) = memory_in_use(pid) {
                members += 1;
                total = total.saturating_add(used);
            }
        }
        (members > 0).then_some(total)
    }
}

/// The process group in the text of a `/proc/<pid>/stat` file (its fifth
/// field). The second field is the command name in parentheses, which may
/// itself hold spaces and parentheses, so the fields are counted from the
/// last `)`: state, parent pid, then the group.
#[cfg(all(unix, not(target_os = "macos")))]
fn proc_stat_pgrp(stat: &str) -> Option<u32> {
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(2)?.parse().ok()
}

/// Called by a capped child ([`run_capped`]) before it touches its input:
/// apply the memory cap its parent passed in [`MEMORY_CAP_ENV`]. On Unix it
/// lowers `RLIMIT_DATA` to the cap (never raises it; best effort on macOS,
/// which does not enforce it and where the parent's watchdog is the cap). On
/// Windows it waits
/// (up to ten seconds) until the parent has put it in a Job Object whose
/// per-process memory limit is at most the cap, and fails otherwise, so it
/// never works uncapped. No variable (a hand-started worker): nothing to do.
pub fn apply_memory_cap() -> Result<(), String> {
    let Some(cap) = std::env::var_os(MEMORY_CAP_ENV) else {
        return Ok(());
    };
    let cap: u64 = cap
        .to_str()
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("{MEMORY_CAP_ENV} is not a byte count"))?;
    #[cfg(unix)]
    {
        // SAFETY: get/setrlimit only read and write the struct they are
        // given.
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::getrlimit(libc::RLIMIT_DATA, &mut limit) } != 0 {
            return Err("cannot read RLIMIT_DATA".into());
        }
        let cap = libc::rlim_t::try_from(cap).unwrap_or(libc::RLIM_INFINITY);
        let lowered = libc::rlimit {
            rlim_cur: limit.rlim_cur.min(cap),
            rlim_max: limit.rlim_max.min(cap),
        };
        let set = unsafe { libc::setrlimit(libc::RLIMIT_DATA, &lowered) } == 0;
        // macOS refuses (EINVAL) to lower RLIMIT_DATA below what the
        // process already maps, and never enforces it on `mmap` anyway: there
        // the parent's footprint watchdog is the cap. Elsewhere it must hold.
        if !set && !cfg!(target_os = "macos") {
            return Err("cannot set RLIMIT_DATA".into());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if job::own_memory_limit().is_some_and(|limit| limit <= cap) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Err("the worker was not placed in its memory-limited job".into())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = cap;
        Ok(())
    }
}

/// The Windows Job Object behind [`run_capped`].
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;

    use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectAssociateCompletionPortInformation,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        JOBOBJECT_ASSOCIATE_COMPLETION_PORT, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    };
    use windows::Win32::System::IO::{
        CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
    };

    /// `JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT` (winnt.h): a process in the job
    /// tried to commit more than the per-process limit.
    const MSG_PROCESS_MEMORY_LIMIT: u32 = 9;

    /// `JOB_OBJECT_MSG_JOB_MEMORY_LIMIT` (winnt.h): the processes in the job
    /// together tried to commit more than the job's limit.
    const MSG_JOB_MEMORY_LIMIT: u32 = 10;

    /// A job whose processes die with the handle and may commit at most a
    /// set number of bytes: each, and only one of them
    /// ([`super::CapScope::Process`]), or all together
    /// ([`super::CapScope::Tree`]). Closing it kills what is in it.
    pub struct MemoryJob {
        job: HANDLE,
        port: HANDLE,
    }

    // SAFETY: the handles are owned by this value and only used through it.
    unsafe impl Send for MemoryJob {}

    impl MemoryJob {
        /// A new job with a memory limit of `cap` bytes over `scope`.
        pub fn new(cap: u64, scope: super::CapScope) -> Result<Self, String> {
            // SAFETY: plain Win32 calls on handles this function owns; the
            // structs passed are the ones the information classes name.
            unsafe {
                let job = CreateJobObjectW(None, None).map_err(|e| e.to_string())?;
                let port = match CreateIoCompletionPort(INVALID_HANDLE_VALUE, None, 0, 1) {
                    Ok(port) => port,
                    Err(e) => {
                        let _ = CloseHandle(job);
                        return Err(e.to_string());
                    }
                };
                let this = Self { job, port };
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                let cap = usize::try_from(cap).unwrap_or(usize::MAX);
                match scope {
                    super::CapScope::Process => {
                        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
                            | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
                        limits.BasicLimitInformation.ActiveProcessLimit = 1;
                        limits.ProcessMemoryLimit = cap;
                    }
                    super::CapScope::Tree => {
                        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY
                            | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
                        limits.JobMemoryLimit = cap;
                    }
                }
                SetInformationJobObject(
                    this.job,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
                .map_err(|e| e.to_string())?;
                let assoc = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
                    CompletionKey: std::ptr::null_mut(),
                    CompletionPort: this.port,
                };
                SetInformationJobObject(
                    this.job,
                    JobObjectAssociateCompletionPortInformation,
                    (&assoc as *const JOBOBJECT_ASSOCIATE_COMPLETION_PORT).cast(),
                    std::mem::size_of::<JOBOBJECT_ASSOCIATE_COMPLETION_PORT>() as u32,
                )
                .map_err(|e| e.to_string())?;
                Ok(this)
            }
        }

        /// Put `child` in the job.
        pub fn assign(&self, child: &std::process::Child) -> Result<(), String> {
            let process = HANDLE(child.as_raw_handle());
            // SAFETY: `process` is the live child's handle, owned by `child`.
            unsafe { AssignProcessToJobObject(self.job, process) }.map_err(|e| e.to_string())
        }

        /// Whether a process in the job hit its memory limit (read from the
        /// job's completion port, without waiting).
        pub fn hit_memory_limit(&self) -> bool {
            loop {
                let mut message = 0u32;
                let mut key = 0usize;
                let mut overlapped: *mut OVERLAPPED = std::ptr::null_mut();
                // SAFETY: out-pointers to locals; a zero timeout never blocks.
                let got = unsafe {
                    GetQueuedCompletionStatus(self.port, &mut message, &mut key, &mut overlapped, 0)
                };
                if got.is_err() {
                    return false;
                }
                if message == MSG_PROCESS_MEMORY_LIMIT || message == MSG_JOB_MEMORY_LIMIT {
                    return true;
                }
            }
        }
    }

    impl Drop for MemoryJob {
        fn drop(&mut self) {
            // SAFETY: both handles are owned here and closed once. Closing
            // the job kills whatever is still in it (KILL_ON_JOB_CLOSE).
            unsafe {
                let _ = CloseHandle(self.job);
                let _ = CloseHandle(self.port);
            }
        }
    }

    /// The memory limit of the job this process is in, if it is in one that
    /// has one: per process or for the whole job, the lower when both.
    pub fn own_memory_limit() -> Option<u64> {
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // SAFETY: a null job handle means "the job of the calling process";
        // the buffer is the struct the class names.
        unsafe {
            QueryInformationJobObject(
                None,
                JobObjectExtendedLimitInformation,
                (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                None,
            )
        }
        .ok()?;
        let flags = limits.BasicLimitInformation.LimitFlags;
        let per_process = (flags & JOB_OBJECT_LIMIT_PROCESS_MEMORY
            == JOB_OBJECT_LIMIT_PROCESS_MEMORY)
            .then_some(limits.ProcessMemoryLimit as u64);
        let whole_job = (flags & JOB_OBJECT_LIMIT_JOB_MEMORY == JOB_OBJECT_LIMIT_JOB_MEMORY)
            .then_some(limits.JobMemoryLimit as u64);
        match (per_process, whole_job) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", script]);
        c
    }

    #[test]
    fn zero_exit_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        run_bounded(sh("exit 0"), "t", &cancel, None, &dir.path().join("e")).unwrap();
    }

    #[test]
    fn non_zero_exit_quotes_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let err = run_bounded(
            sh("echo boom-detail >&2; exit 3"),
            "Tool",
            &cancel,
            None,
            &dir.path().join("e"),
        )
        .unwrap_err();
        match err {
            EngineError::ConversionFailed(m) => {
                assert!(m.contains("Tool failed"), "{m}");
                assert!(m.contains("boom-detail"), "{m}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    /// A converter's failure message quotes what it said, minus the noise it
    /// prints on every run: a corrupt DOCX used to report six Fontconfig
    /// warnings ahead of LibreOffice's one meaningful line.
    #[test]
    fn fr_053_failure_detail_drops_converter_noise() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let script = "for i in 1 2 3 4 5 6; do \
              echo 'Fontconfig warning: no <cachedir> elements found. Check configuration.' >&2; \
            done; \
            echo '(soffice:1): Gtk-WARNING **: cannot open display' >&2; \
            echo 'Error: source file could not be loaded' >&2; exit 1";
        let err = run_bounded(
            sh(script),
            "LibreOffice",
            &cancel,
            None,
            &dir.path().join("e"),
        )
        .unwrap_err();
        let EngineError::ConversionFailed(m) = err else {
            panic!("unexpected {err:?}");
        };
        assert!(
            m.ends_with(": Error: source file could not be loaded"),
            "{m}"
        );
        assert!(!m.contains("Fontconfig"), "{m}");
        assert!(!m.contains("Gtk-WARNING"), "{m}");
    }

    /// An OS error while waiting is not a timeout and must not read like one
    /// ("did not finish within 0s").
    #[test]
    fn a_failed_wait_is_reported_with_its_cause_not_as_a_timeout() {
        let err = stopped_error(
            Stopped::WaitFailed("Interrupted system call".to_string()),
            "Pandoc",
            None,
        );
        match err {
            EngineError::ConversionFailed(m) => {
                assert!(m.contains("Interrupted system call"), "{m}");
                assert!(!m.contains("did not finish"), "{m}");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            stopped_error(Stopped::TimedOut, "Pandoc", Some(Duration::from_secs(9))),
            EngineError::ConversionTimedOut(m) if m.contains("9s")
        ));
    }

    #[test]
    fn timeout_kills_the_child_and_its_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        // The background `sleep` is a helper in the same process group; if the
        // group kill missed it, it would create the marker after 2 s.
        let script = format!("(sleep 2; touch '{}') & sleep 30", marker.display());
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let err = run_bounded(
            sh(&script),
            "Slow",
            &cancel,
            Some(Duration::from_millis(300)),
            &dir.path().join("e"),
        )
        .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "must not wait 30 s"
        );
        // A timeout is its own (terminal) error, so the queue does not retry
        // a converter that already ran out of time (FR-034).
        assert!(
            matches!(&err, EngineError::ConversionTimedOut(m) if m.contains("did not finish")),
            "{err:?}"
        );
        std::thread::sleep(Duration::from_millis(2500));
        assert!(!marker.exists(), "a helper outlived the kill");
    }

    #[test]
    fn cancel_flag_kills_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            flag.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        let err =
            run_bounded(sh("sleep 30"), "t", &cancel, None, &dir.path().join("e")).unwrap_err();
        assert!(matches!(err, EngineError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn fr_055_a_child_that_never_exits_is_done_once_its_output_is_finished() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        // Writes its output, then lingers like headless Chrome does.
        let script = format!("echo done > '{}'; sleep 30", out.display());
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let done = || out.is_file();
        run_until(
            sh(&script),
            "Lingering",
            &cancel,
            Some(Duration::from_secs(20)),
            &dir.path().join("e"),
            Some(&done),
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_program_that_does_not_exist_is_a_missing_dependency() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let err = run_bounded(
            Command::new(dir.path().join("no-such-binary")),
            "Ghost",
            &cancel,
            None,
            &dir.path().join("e"),
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::DependencyMissing(_)), "{err:?}");
    }

    #[test]
    fn arguments_are_never_shell_interpolated() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("argv");
        let hostile = "$(touch pwned); `touch pwned2`";
        let mut cmd = Command::new("/bin/sh");
        // $1 is written verbatim; the hostile text is an argument, not code.
        cmd.current_dir(dir.path()).args([
            "-c",
            "printf '%s' \"$1\" > \"$2\"",
            "sh",
            hostile,
            out.to_str().unwrap(),
        ]);
        let cancel = AtomicBool::new(false);
        run_bounded(cmd, "t", &cancel, None, &dir.path().join("e")).unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), hostile);
        assert!(!dir.path().join("pwned").exists());
        assert!(!dir.path().join("pwned2").exists());
    }
}

#[cfg(test)]
mod reason_tests {
    use super::*;

    /// FR-092: every way a converter process fails carries a registered
    /// reason and the params its sentence needs, so the queue says it in the
    /// user's language instead of quoting the English detail.
    #[test]
    fn fr_092_engine_failures_carry_reason_for_converter_processes() {
        let gone = std::io::Error::from(std::io::ErrorKind::NotFound);
        match spawn_error("LibreOffice", &gone) {
            EngineError::DependencyMissing(f) => {
                assert_eq!(f.reason, Some("libre-office"));
                assert!(f.contains("LibreOffice could not be started"), "{f}");
            }
            other => panic!("unexpected {other:?}"),
        }
        // An unknown label still pauses as a missing dependency, reasonless.
        assert!(matches!(
            spawn_error("Ghost", &gone),
            EngineError::DependencyMissing(f) if f.reason.is_none()
        ));
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        match spawn_error("Pandoc", &denied) {
            EngineError::ConversionFailed(f) => {
                assert_eq!(f.reason, Some("converter_start_failed"));
                assert_eq!(f.params["tool"], "Pandoc");
            }
            other => panic!("unexpected {other:?}"),
        }
        match stopped_error(Stopped::TimedOut, "sips", Some(Duration::from_secs(9))) {
            EngineError::ConversionTimedOut(f) => {
                assert_eq!(f.reason, Some("tool_timed_out"));
                assert_eq!(f.params["tool"], "sips");
                assert_eq!(f.params["secs"], "9");
            }
            other => panic!("unexpected {other:?}"),
        }
        match stopped_error(Stopped::WaitFailed("EINTR".into()), "sips", None) {
            EngineError::ConversionFailed(f) => {
                assert_eq!(f.reason, Some("converter_crashed"));
                assert!(f.contains("EINTR"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    /// Every label a converter is run under maps to a registered dependency.
    #[test]
    fn fr_058_known_converter_labels_map_to_registered_dependency_ids() {
        for label in ["LibreOffice", "Pandoc", "sips"] {
            let id = dependency_id_of(label).expect(label);
            assert!(
                crate::util::reasons::is_job_reason("dependency_missing", id),
                "{id}"
            );
        }
    }

    /// Security note 8: the watchdog reads a child's memory, and a capped
    /// child that grows past its cap is killed as `OverMemory`; one that
    /// stays under it runs to the end; cancel and timeout behave as in
    /// `run_bounded`.
    /// The group probe and the group kill address the child's group, never
    /// ours. procps-ng's `kill` (Ubuntu 22.04) reads `kill -0 -<pgid>` as two
    /// signal options and probes, or kills, the caller's own group: the
    /// probe then said every finished tree was alive, and the kill took the
    /// test runner with it.
    #[cfg(unix)]
    #[test]
    fn kill_tree_ends_the_childs_group_and_the_probe_sees_it_gone() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "sleep 30 & sleep 30"]);
        isolate_process_tree(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let pgid = child.id();
        assert_eq!(group_alive(pgid), Some(true));
        assert!(kill_tree_confirmed(&mut child));
        // We are still here, and the group really is gone.
        assert_eq!(group_alive(pgid), Some(false));
    }

    /// The group is read from the fields after the command name, whatever
    /// that name holds.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn proc_stat_pgrp_reads_the_group_after_the_command_name() {
        let stat = "4242 (a) b (c) S 4100 4200 4100 0 -1 4194560 120 0 0 0";
        assert_eq!(proc_stat_pgrp(stat), Some(4200));
        assert_eq!(proc_stat_pgrp("4242 (sh) S 1 4242 1"), Some(4242));
        assert_eq!(proc_stat_pgrp("4242 (sh"), None);
        assert_eq!(proc_stat_pgrp(""), None);
        // This process's own group, as the kernel reports it.
        let own = std::fs::read_to_string("/proc/self/stat").unwrap();
        // SAFETY: getpgrp has no preconditions.
        let expected = u32::try_from(unsafe { libc::getpgrp() }).unwrap();
        assert_eq!(proc_stat_pgrp(&own), Some(expected));
    }

    #[cfg(unix)]
    #[test]
    fn nfr_security_run_capped_kills_a_child_over_its_memory_cap() {
        let sh = |script: &str| {
            let mut c = Command::new("/bin/sh");
            c.args(["-c", script]);
            c
        };
        assert!(memory_in_use(std::process::id()).is_some_and(|b| b > 0));
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("err.log");
        let cancel = AtomicBool::new(false);
        // `perl` ships with macOS and every Linux CI image: grow a string to
        // ~400 MB, touching every page.
        let grow = "exec perl -e '$x = q(a) x (400*1024*1024); sleep 30'";
        let started = Instant::now();
        let r = run_capped(
            sh(grow),
            "grow",
            &cancel,
            None,
            &log,
            64 * 1024 * 1024,
            CapScope::Process,
        );
        assert!(matches!(r, Err(CappedError::OverMemory)), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(20));
        // Round 7: a tree cap also counts a helper the child starts (the
        // shell stays, perl runs as its child in the same group).
        // On Linux that needs every member of the group summed: counting the
        // leader alone let the shell sleep out perl's 30 s and exit 0.
        let helper = "perl -e '$x = q(a) x (400*1024*1024); sleep 30'; exit 0";
        let started = Instant::now();
        let r = run_capped(
            sh(helper),
            "grow",
            &cancel,
            None,
            &log,
            64 * 1024 * 1024,
            CapScope::Tree,
        );
        assert!(matches!(r, Err(CappedError::OverMemory)), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(20));

        assert!(run_capped(
            sh("exit 0"),
            "ok",
            &cancel,
            None,
            &log,
            64 << 20,
            CapScope::Process
        )
        .is_ok());
        let r = run_capped(
            sh("exit 3"),
            "bad",
            &cancel,
            None,
            &log,
            64 << 20,
            CapScope::Process,
        );
        assert!(
            matches!(&r, Err(CappedError::Engine(EngineError::ConversionFailed(f)))
                if f.reason == Some("converter_failed")),
            "{r:?}"
        );
        let r = run_capped(
            sh("sleep 30"),
            "slow",
            &cancel,
            Some(Duration::from_millis(200)),
            &log,
            64 << 20,
            CapScope::Process,
        );
        assert!(
            matches!(
                &r,
                Err(CappedError::Engine(EngineError::ConversionTimedOut(_)))
            ),
            "{r:?}"
        );
        let cancelled = AtomicBool::new(true);
        let r = run_capped(
            sh("sleep 30"),
            "c",
            &cancelled,
            None,
            &log,
            64 << 20,
            CapScope::Process,
        );
        assert!(
            matches!(r, Err(CappedError::Engine(EngineError::Cancelled))),
            "{r:?}"
        );

        // A refused allocation (what the Job Object or RLIMIT_DATA end in)
        // reads as out of memory.
        std::fs::write(&log, "memory allocation of 1048576 bytes failed\n").unwrap();
        assert!(allocation_failed(&log));
        std::fs::write(&log, "panicked at src/x.rs\n").unwrap();
        assert!(!allocation_failed(&log));
    }
}
