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
///
/// On Linux the child is also made the subreaper of everything it starts
/// (`PR_SET_CHILD_SUBREAPER`, kept across `exec`): a helper whose parent
/// exits (a double fork) is adopted by the child instead of by init, so it
/// stays the child's descendant and [`tree::TreeSampler`] still finds it by
/// its parent link, even after a `setsid` took it out of the group.
pub(crate) fn isolate_process_tree(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: runs in the forked child before exec; prctl is a single
        // async-signal-safe syscall that touches no memory of ours. Best
        // effort: a kernel without it (before 3.4) leaves the old
        // behaviour, an orphan adopted by init.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
                Ok(())
            });
        }
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

/// On Linux, how many [`MEMORY_POLL`]s pass between two walks of `/proc` that
/// refresh which processes belong to a [`CapScope::Tree`] child
/// ([`tree::TreeSampler`]): the walk reads every `/proc/<pid>/stat` on the
/// system, a few milliseconds on a busy desktop. The memory of every process
/// already known to be in the tree is still read on every poll; only a
/// helper started since the last walk waits for the next one (at most
/// 200 ms) to be counted.
#[cfg(all(unix, not(target_os = "macos")))]
const TREE_SCAN_EVERY: u32 = 10;

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
///   [`CapScope::Tree`] it samples more than the child:
///   - *macOS:* the child's whole process group ([`group_memory_in_use`]).
///     WebKit's own web-content and networking processes are XPC services
///     that launchd starts, outside that group: they are not covered.
///   - *Linux:* the child's process group **and** every descendant of the
///     child or of a group member, found by their parent links
///     ([`tree::TreeSampler`]), so a helper that starts its own session or
///     group (`setsid`, `bwrap --new-session`) is still counted. Which
///     processes belong is refreshed every [`TREE_SCAN_EVERY`] polls; the
///     resident set of each one already known is read on every poll, so a
///     known helper cannot grow past the cap between two refreshes. The
///     child is the subreaper of its tree ([`isolate_process_tree`]), so a
///     helper whose parent exits (a double fork) is adopted by the child
///     and found by the next walk while the child lives. A
///     process stays a member once found, even after its parent exits,
///     until it exits itself; a pid the kernel reused for
///     another process is told apart by its start time. Past the cap, on a
///     cancel or a timeout, the tree is frozen (every member stopped until
///     no new one appears) and then every member is killed, as well as the
///     group; once the child has exited the members it left behind are
///     killed the same way, as the Job Object does on Windows.
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
    let _ = &cannot_cap;

    let started = Instant::now();
    let mut watch = Watch::new(child.id(), scope);
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            watch.kill(&mut child);
            return Err(engine(EngineError::Cancelled));
        }
        if timeout.is_some_and(|limit| started.elapsed() >= limit) {
            watch.kill(&mut child);
            return Err(engine(timed_out_error(label, timeout)));
        }
        if watch.used().is_some_and(|used| used > memory_cap) {
            watch.kill(&mut child);
            return Err(CappedError::OverMemory);
        }
        match child.wait_timeout(MEMORY_POLL) {
            Ok(Some(status)) => break status,
            Ok(None) => continue,
            Err(e) => {
                watch.kill(&mut child);
                return Err(engine(stopped_error(
                    Stopped::WaitFailed(e.to_string()),
                    label,
                    timeout,
                )));
            }
        }
    };
    watch.exited();
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
/// grow past the cap unseen. `None` when the group is gone. macOS only:
/// Linux counts a tree, not a group ([`tree::TreeSampler`]).
#[cfg(target_os = "macos")]
pub fn group_memory_in_use(pgid: u32) -> Option<u64> {
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

/// What [`run_capped`] watches while its child runs, by platform and
/// [`CapScope`], and how it ends the child and what it started.
struct Watch {
    #[cfg(unix)]
    leader: u32,
    #[cfg(unix)]
    scope: CapScope,
    #[cfg(all(unix, not(target_os = "macos")))]
    tree: Option<tree::TreeSampler>,
}

impl Watch {
    fn new(leader: u32, scope: CapScope) -> Self {
        #[cfg(not(unix))]
        let _ = (leader, scope);
        Self {
            #[cfg(unix)]
            leader,
            #[cfg(unix)]
            scope,
            #[cfg(all(unix, not(target_os = "macos")))]
            tree: (scope == CapScope::Tree).then(|| tree::TreeSampler::new(leader)),
        }
    }

    /// The memory in use now, as the scope counts it. `None` when it cannot
    /// be read, and always on Windows, where the Job Object enforces the cap
    /// itself.
    fn used(&mut self) -> Option<u64> {
        #[cfg(target_os = "macos")]
        {
            match self.scope {
                CapScope::Process => memory_in_use(self.leader),
                CapScope::Tree => group_memory_in_use(self.leader),
            }
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let _ = self.scope;
            match &mut self.tree {
                Some(tree) => tree.sample(),
                None => memory_in_use(self.leader),
            }
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// End the running child and everything it started, then reap it. On
    /// Linux, for a tree, each member is killed before the group: the
    /// child's parent links are only whole while it is alive.
    fn kill(&mut self, child: &mut Child) {
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(tree) = &mut self.tree {
            tree.kill_all();
        }
        kill_tree(child);
    }

    /// The child exited by itself (and was reaped): on Linux, for a tree,
    /// kill the helpers it left behind, as closing the Job Object does on
    /// Windows. Nothing on macOS, where it never did.
    fn exited(&mut self) {
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(tree) = &mut self.tree {
            tree.leader_reaped();
            tree.kill_all();
        }
    }
}

/// The Linux side of a [`CapScope::Tree`] cap: which processes belong to the
/// capped child's tree, how much memory they use, and how they are all
/// killed. Everything is read from `/proc`.
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) mod tree {
    use std::collections::{HashMap, HashSet};

    use super::TREE_SCAN_EVERY;

    /// The fields of a `/proc/<pid>/stat` file the tree is built from.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct ProcStat {
        /// Parent pid (field 4).
        pub ppid: u32,
        /// Process group (field 5).
        pub pgrp: u32,
        /// Start time after boot, in clock ticks (field 22): with the pid,
        /// it names one process, never the next one the pid is given to.
        pub start: u64,
        /// Resident set, in pages (field 24).
        pub rss_pages: u64,
    }

    /// The [`ProcStat`] in the text of a `/proc/<pid>/stat` file. The second
    /// field is the command name in parentheses, which may itself hold
    /// spaces and parentheses, so the fields are counted from the last `)`.
    /// Pure.
    pub(crate) fn parse_stat(stat: &str) -> Option<ProcStat> {
        let (_, rest) = stat.rsplit_once(')')?;
        // `rest` starts at field 3 (the state).
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let field = |n: usize| fields.get(n - 3)?.parse::<u64>().ok();
        Some(ProcStat {
            ppid: u32::try_from(field(4)?).ok()?,
            pgrp: u32::try_from(field(5)?).ok()?,
            start: field(22)?,
            rss_pages: field(24)?,
        })
    }

    /// `/proc/<pid>/stat`, parsed; `None` once the process is gone.
    pub(crate) fn read_stat(pid: u32) -> Option<ProcStat> {
        parse_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
    }

    fn page_size() -> u64 {
        // SAFETY: sysconf has no preconditions.
        u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(4096)
    }

    /// One process of the tree: its pid and the start time it had when it
    /// was found, so a reused pid is never taken for it.
    pub(crate) type Member = (u32, u64);

    /// The processes of a capped child's tree and the memory they use
    /// (`run_capped`'s docs). Members are found by walking all of `/proc`
    /// every [`TREE_SCAN_EVERY`] samples; each known member's resident set
    /// is read on every sample, one `/proc/<pid>/stat` each.
    pub(crate) struct TreeSampler {
        leader: u32,
        /// Whether `leader` still names the child (and its process group):
        /// true until the child has been reaped. After that its pid may be
        /// given to an unrelated process, which may lead a group of its own.
        leader_ours: bool,
        samples: u32,
        members: Vec<Member>,
        page: u64,
    }

    impl TreeSampler {
        pub(crate) fn new(leader: u32) -> Self {
            Self {
                leader,
                leader_ours: true,
                samples: 0,
                members: Vec::new(),
                page: page_size(),
            }
        }

        /// The members found so far.
        #[cfg(test)]
        pub(crate) fn members(&self) -> &[Member] {
            &self.members
        }

        /// The memory the whole tree uses now, in bytes; `None` when none of
        /// it is left. Refreshes the membership every [`TREE_SCAN_EVERY`]
        /// calls, the first included.
        pub(crate) fn sample(&mut self) -> Option<u64> {
            let scan = self.samples.is_multiple_of(TREE_SCAN_EVERY);
            self.samples = self.samples.wrapping_add(1);
            if scan {
                return self.refresh();
            }
            let mut total = 0u64;
            self.members.retain(|&(pid, start)| match read_stat(pid) {
                Some(stat) if stat.start == start => {
                    total = total.saturating_add(stat.rss_pages);
                    true
                }
                // Gone, or the pid now names another process.
                _ => false,
            });
            (!self.members.is_empty()).then(|| total.saturating_mul(self.page))
        }

        /// Walk `/proc` and recompute the members: the leader and its group
        /// (while [`Self::leader_ours`]), every member already known that is
        /// still the same process, and every descendant of any of them.
        /// Returns the memory they use, read in the same walk.
        pub(crate) fn refresh(&mut self) -> Option<u64> {
            let Ok(dir) = std::fs::read_dir("/proc") else {
                // No /proc to walk: keep the members known so far.
                return None;
            };
            let all: HashMap<u32, ProcStat> = dir
                .flatten()
                .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
                .filter_map(|pid| Some((pid, read_stat(pid)?)))
                .collect();
            let known: HashSet<Member> = self.members.iter().copied().collect();
            let mut found: Vec<u32> = all
                .iter()
                .filter(|(pid, stat)| {
                    known.contains(&(**pid, stat.start))
                        || (self.leader_ours && (**pid == self.leader || stat.pgrp == self.leader))
                })
                .map(|(pid, _)| *pid)
                .collect();
            let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
            for (pid, stat) in &all {
                children.entry(stat.ppid).or_default().push(*pid);
            }
            let mut seen: HashSet<u32> = found.iter().copied().collect();
            let mut next = 0;
            while next < found.len() {
                for &child in children.get(&found[next]).map(Vec::as_slice).unwrap_or(&[]) {
                    if seen.insert(child) {
                        found.push(child);
                    }
                }
                next += 1;
            }
            let mut total = 0u64;
            self.members = found
                .into_iter()
                .map(|pid| {
                    let stat = all[&pid];
                    total = total.saturating_add(stat.rss_pages);
                    (pid, stat.start)
                })
                .collect();
            (!self.members.is_empty()).then(|| total.saturating_mul(self.page))
        }

        /// The child has been reaped: its pid no longer names it, nor its
        /// group, unless the group still has members (the kernel does not
        /// give out a pid that is a live group's id). When the pid already
        /// names another process, the group is gone and only the members
        /// known so far and their descendants are the tree.
        pub(crate) fn leader_reaped(&mut self) {
            self.members.retain(|&(pid, _)| pid != self.leader);
            self.leader_ours = read_stat(self.leader).is_none();
        }

        /// Freeze the whole tree, then kill it: SIGSTOP every member,
        /// refreshing the membership after each round until a walk finds no
        /// member not yet stopped (at most [`FREEZE_ROUNDS`] walks), then
        /// SIGKILL every member stopped, the leader last.
        ///
        /// A stopped process starts nothing, and a process whose parent is
        /// stopped keeps that parent, so each walk finds whatever was forked
        /// before the last stop by its parent link, and the tree settles.
        /// Killing round by round instead left a gap: a member killed while
        /// it was forking orphaned the child it had just started, which then
        /// belonged to no one we knew (a `setsid` child has left the group
        /// too) and survived.
        pub(crate) fn kill_all(&mut self) {
            let mut stopped: Vec<Member> = Vec::new();
            let mut seen: HashSet<Member> = HashSet::new();
            for _ in 0..FREEZE_ROUNDS {
                self.refresh();
                let fresh: Vec<Member> = self
                    .members
                    .iter()
                    .filter(|m| !seen.contains(*m))
                    .copied()
                    .collect();
                if fresh.is_empty() {
                    break;
                }
                for member in fresh {
                    signal_member(member, libc::SIGSTOP);
                    seen.insert(member);
                    stopped.push(member);
                }
            }
            let (leader, rest): (Vec<Member>, Vec<Member>) = stopped
                .into_iter()
                .partition(|&(pid, _)| self.leader_ours && pid == self.leader);
            for member in rest.into_iter().chain(leader) {
                signal_member(member, libc::SIGKILL);
            }
        }
    }

    /// How many walks of `/proc` [`TreeSampler::kill_all`] makes, at most,
    /// while it stops the tree: one finds nothing new once every member is
    /// stopped, so this bounds only a tree that keeps out-forking the walk.
    const FREEZE_ROUNDS: usize = 32;

    /// Whether [`signal_member`] falls back to a checked `kill` after
    /// `pidfd_open` failed with `errno`: when the kernel has no pidfds
    /// (`ENOSYS`, before 5.3) or none can be had now (`EMFILE`, `ENFILE`,
    /// `ENOMEM`). A member stopped by the freeze and then left unsignalled
    /// would stay frozen for good. Only `ESRCH` (already gone) and the
    /// like end there. Pure.
    pub(crate) fn falls_back_to_kill(errno: Option<i32>) -> bool {
        matches!(
            errno,
            Some(libc::ENOSYS | libc::EMFILE | libc::ENFILE | libc::ENOMEM)
        )
    }

    /// Send `signal` to `pid` with a plain `kill` if it is still the
    /// process that started at `start`, checked just before (a pid reused
    /// between the check and the signal is the one gap left).
    pub(crate) fn kill_checked((pid, start): Member, signal: libc::c_int) {
        let Ok(raw) = libc::pid_t::try_from(pid) else {
            return;
        };
        if read_stat(pid).is_some_and(|stat| stat.start == start) {
            // SAFETY: kill has no memory preconditions.
            unsafe { libc::kill(raw, signal) };
        }
    }

    /// Send `signal` to `pid` if it is still the process that started at
    /// `start`. Through a pidfd where the kernel has one (Linux 5.3): the
    /// descriptor pins the process, so the check and the signal are about
    /// the same one even if it exits in between; otherwise, and when no
    /// descriptor can be had ([`falls_back_to_kill`]), [`kill_checked`].
    pub(crate) fn signal_member((pid, start): Member, signal: libc::c_int) {
        let Ok(raw) = libc::pid_t::try_from(pid) else {
            return;
        };
        let same = || read_stat(pid).is_some_and(|stat| stat.start == start);
        #[cfg(target_os = "linux")]
        {
            // SAFETY: plain syscalls on integers; the descriptor returned is
            // ours (close-on-exec by default) and closed once, below.
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, raw, 0) };
            if fd >= 0 {
                if same() {
                    // SAFETY: as above; a null siginfo means a plain kill.
                    unsafe {
                        libc::syscall(
                            libc::SYS_pidfd_send_signal,
                            fd,
                            signal,
                            std::ptr::null::<libc::siginfo_t>(),
                            0,
                        );
                    }
                }
                // SAFETY: `fd` is the descriptor opened above.
                unsafe { libc::close(fd as libc::c_int) };
                return;
            }
            if !falls_back_to_kill(std::io::Error::last_os_error().raw_os_error()) {
                // ESRCH: already gone.
                return;
            }
        }
        kill_checked((pid, start), signal);
    }
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

    /// The fields are read after the command name, whatever that name
    /// holds.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn proc_stat_fields_are_read_after_the_command_name() {
        let stat = "4242 (a) b (c) S 4100 4200 4100 0 -1 4194560 120 0 0 0 \
                    7 3 0 0 20 0 1 0 987654 10485760 2560 18446744073709551615";
        assert_eq!(
            tree::parse_stat(stat),
            Some(tree::ProcStat {
                ppid: 4100,
                pgrp: 4200,
                start: 987654,
                rss_pages: 2560,
            })
        );
        assert_eq!(tree::parse_stat("4242 (sh) S 1 4242 1"), None);
        assert_eq!(tree::parse_stat("4242 (sh"), None);
        assert_eq!(tree::parse_stat(""), None);
        // This process, as the kernel reports it.
        let own = tree::read_stat(std::process::id()).unwrap();
        // SAFETY: getpgrp and getppid have no preconditions.
        let (pgrp, ppid) = unsafe { (libc::getpgrp(), libc::getppid()) };
        assert_eq!(own.pgrp, u32::try_from(pgrp).unwrap());
        assert_eq!(own.ppid, u32::try_from(ppid).unwrap());
        assert!(own.rss_pages > 0 && own.start > 0);
    }

    /// Waits up to `limit` for `path` to exist and hold something.
    #[cfg(all(unix, not(target_os = "macos")))]
    fn wait_for_file(path: &Path, limit: Duration) -> String {
        let deadline = Instant::now() + limit;
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if !text.trim().is_empty() {
                    return text;
                }
            }
            assert!(Instant::now() < deadline, "{} never came", path.display());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// What a Linux tree test started, SIGKILLed if the test panics (a
    /// failed assertion included), so a failure never leaves a `sleep 600`
    /// behind: the process whose pid the test's `pid_file` holds (an
    /// orphan, read when the guard drops, so a panic before the test read
    /// it is covered), the process `group`, and the `child`, the latter
    /// checked against its start time so a pid the kernel reused is left
    /// alone. A test that passes cleans up itself and asserts it did.
    #[cfg(target_os = "linux")]
    #[derive(Default)]
    struct KillOnPanic {
        pid_file: Option<std::path::PathBuf>,
        group: Option<u32>,
        child: Option<tree::Member>,
    }

    #[cfg(target_os = "linux")]
    impl KillOnPanic {
        /// Guard `child` (pid and start time).
        fn child(mut self, pid: u32) -> Self {
            self.child = tree::read_stat(pid).map(|stat| (pid, stat.start));
            self
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for KillOnPanic {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                return;
            }
            // Never 0 or 1: kill(0) is our own group, kill(1) init.
            let real = |pid: u32| pid > 1 && i32::try_from(pid).is_ok();
            let orphan = self
                .pid_file
                .as_ref()
                .and_then(|f| std::fs::read_to_string(f).ok())
                .and_then(|s| s.trim().parse::<u32>().ok())
                .filter(|&pid| real(pid));
            if let Some(pid) = orphan {
                // SAFETY: a plain syscall on a positive pid.
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            }
            if let Some(group) = self.group.filter(|&g| real(g)) {
                // SAFETY: a plain syscall on another process group.
                unsafe { libc::killpg(group as i32, libc::SIGKILL) };
            }
            if let Some(member) = self.child {
                tree::kill_checked(member, libc::SIGKILL);
            }
        }
    }

    /// Whether `pid` is a live process (not gone, not a zombie), checked for
    /// up to two seconds while it is.
    #[cfg(all(unix, not(target_os = "macos")))]
    fn still_alive(pid: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let alive = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|s| Some(s.rsplit_once(')')?.1.split_whitespace().next()? != "Z"))
                .unwrap_or(false);
            if !alive || Instant::now() >= deadline {
                return alive;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Security note 8 on Linux: a helper found by one walk of `/proc` is
    /// measured on every poll after it, not only at the next walk. The
    /// helper here waits until a walk has found it, then grows by 300 MB;
    /// the very next sample (not a walk) must see it. The sampler this
    /// replaced read only the leader between walks and added what the
    /// others used at the last one, so for up to nine polls a helper could
    /// grow unseen.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn nfr_security_a_known_helper_that_grows_right_after_a_walk_is_seen_at_the_next_poll() {
        const { assert!(TREE_SCAN_EVERY > 2, "the second sample must not be a walk") };
        let dir = tempfile::tempdir().unwrap();
        let (pid_file, go, grown) = (
            dir.path().join("pid"),
            dir.path().join("go"),
            dir.path().join("grown"),
        );
        // The size is an argument, not a literal: perl folds a constant `x`
        // while it compiles, and would grow before it ever waited.
        let helper = format!(
            "perl -e 'open my $f, q(>), $ARGV[0]; print $f $$; close $f; \
               select(undef, undef, undef, 0.01) until -e $ARGV[1]; \
               $x = q(a) x ($ARGV[3]*1024*1024); open $f, q(>), $ARGV[2]; print $f 1; close $f; \
               sleep 30' '{}' '{}' '{}' 300 & wait",
            pid_file.display(),
            go.display(),
            grown.display()
        );
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", &helper]);
        isolate_process_tree(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let perl: u32 = wait_for_file(&pid_file, Duration::from_secs(10))
            .trim()
            .parse()
            .unwrap();

        let mut tree = tree::TreeSampler::new(child.id());
        let before = tree.sample().expect("the walk finds the tree");
        assert!(
            tree.members().iter().any(|&(pid, _)| pid == perl),
            "the walk found the helper: {:?}",
            tree.members()
        );
        std::fs::write(&go, "1").unwrap();
        wait_for_file(&grown, Duration::from_secs(20));
        let after = tree.sample().expect("the tree is still there");
        assert!(
            after >= before + 250 * 1024 * 1024,
            "the next poll saw {after} bytes, {before} before the helper grew"
        );

        tree.kill_all();
        kill_tree(&mut child);
        assert!(!still_alive(perl), "the helper outlived the kill");
    }

    /// Security note 8 on Linux: a helper orphaned by a double fork, out of
    /// the group too (`setsid`), is adopted by the capped child (its
    /// subreaper), so a walk after the orphaning still finds it by its
    /// parent link, and it is killed with the tree. Adopted by init, it was
    /// no one's descendant we knew and outlived the kill.
    #[cfg(target_os = "linux")]
    #[test]
    fn nfr_security_a_double_forked_orphan_stays_in_the_tree_and_dies_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("orphan");
        // The subshell starts the helper and exits at once, orphaning it.
        let script = format!(
            "(setsid sleep 600 & echo $! > '{}'); sleep 30",
            pid_file.display()
        );
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", &script]);
        isolate_process_tree(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let leader = child.id();
        let _guard = KillOnPanic {
            pid_file: Some(pid_file.clone()),
            group: Some(leader),
            ..KillOnPanic::default()
        }
        .child(leader);
        let orphan: u32 = wait_for_file(&pid_file, Duration::from_secs(10))
            .trim()
            .parse()
            .unwrap();
        // Adopted once its parent subshell has gone.
        let deadline = Instant::now() + Duration::from_secs(5);
        let stat = loop {
            let stat = tree::read_stat(orphan).expect("the orphan is alive");
            if stat.ppid == leader || Instant::now() >= deadline {
                break stat;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(stat.ppid, leader, "the orphan was not adopted by the child");
        assert_ne!(stat.pgrp, leader, "setsid did not move the orphan out");

        // A first walk made after the orphaning finds it.
        let mut tree = tree::TreeSampler::new(leader);
        tree.sample().expect("the walk finds the tree");
        assert!(
            tree.members().iter().any(|&(pid, _)| pid == orphan),
            "{:?}",
            tree.members()
        );
        tree.kill_all();
        kill_tree(&mut child);
        assert!(!still_alive(orphan), "the orphan outlived the kill");
    }

    /// A member is signalled even when no pidfd can be had (`EMFILE`,
    /// `ENFILE`: too many open files), through a `kill` checked against its
    /// start time, so a member the freeze stopped is never left frozen; a
    /// pid that names another process now is left alone.
    #[cfg(target_os = "linux")]
    #[test]
    fn nfr_security_a_member_is_signalled_without_a_pidfd() {
        for errno in [libc::ENOSYS, libc::EMFILE, libc::ENFILE, libc::ENOMEM] {
            assert!(tree::falls_back_to_kill(Some(errno)), "{errno}");
        }
        assert!(!tree::falls_back_to_kill(Some(libc::ESRCH)));
        assert!(!tree::falls_back_to_kill(None));

        let mut child = Command::new("sleep").arg("600").spawn().unwrap();
        let pid = child.id();
        let _guard = KillOnPanic::default().child(pid);
        let start = tree::read_stat(pid).unwrap().start;
        // Another start time: not this process, not signalled.
        tree::kill_checked((pid, start + 1), libc::SIGKILL);
        assert!(child.try_wait().unwrap().is_none());
        tree::kill_checked((pid, start), libc::SIGSTOP);
        tree::kill_checked((pid, start), libc::SIGKILL);
        let status = child.wait().unwrap();
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(libc::SIGKILL)
        );
    }

    /// Security note 8 on Linux: a member that keeps forking children into
    /// sessions of their own is killed with every child it managed to start.
    /// Killed round by round, the looping member died between two forks and
    /// orphaned the child it had just started, outside the group and no
    /// longer anyone's descendant we knew; frozen first, it keeps its
    /// children until all of them are found and killed.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn nfr_security_kill_all_leaves_no_survivor_of_a_member_forking_in_a_loop() {
        let dir = tempfile::tempdir().unwrap();
        let pids = dir.path().join("pids");
        let looper = dir.path().join("looper");
        // Forks `setsid sleep` children as fast as it can, recording each
        // one; the shell around it leads the group.
        let script = format!(
            "sh -c 'echo $$ > \"$1\"; while :; do setsid sleep 600 & echo $! >> \"$2\"; done' \
               sh '{}' '{}' & wait",
            looper.display(),
            pids.display()
        );
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", &script]);
        isolate_process_tree(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let looper: u32 = wait_for_file(&looper, Duration::from_secs(10))
            .trim()
            .parse()
            .unwrap();
        let mut tree = tree::TreeSampler::new(child.id());
        tree.sample().expect("the walk finds the tree");
        // Let it fork well past one walk's worth of children.
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::fs::read_to_string(&pids).map_or(0, |p| p.lines().count()) < 50 {
            assert!(Instant::now() < deadline, "the loop never forked");
            std::thread::sleep(Duration::from_millis(20));
        }

        tree.kill_all();
        kill_tree(&mut child);

        assert!(!still_alive(looper), "the forking member outlived kill_all");
        let started: Vec<u32> = std::fs::read_to_string(&pids)
            .unwrap()
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect();
        // SIGKILL is delivered at once, but give the kernel a moment.
        let alive = |pid: u32| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|s| Some(s.rsplit_once(')')?.1.split_whitespace().next()? != "Z"))
                .unwrap_or(false)
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        let survivors = loop {
            let survivors: Vec<u32> = started.iter().copied().filter(|&p| alive(p)).collect();
            if survivors.is_empty() || Instant::now() >= deadline {
                break survivors;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(
            survivors.is_empty(),
            "{} of {} children survived: {survivors:?}",
            survivors.len(),
            started.len()
        );
    }

    /// Security note 8 on Linux: a helper that leaves the child's process
    /// group and session (`setsid`, as `bwrap --new-session` does for a
    /// sandboxed web process) is still the child's descendant: it counts
    /// against the tree cap and is killed with the tree. It used to escape
    /// both: the group scan never saw it, and the group kill never reached
    /// it. A helper left behind by a child that exits by itself is killed
    /// too, as closing the Job Object does on Windows.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn nfr_security_a_setsid_helper_counts_against_the_tree_cap_and_dies_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("err.log");
        let cancel = AtomicBool::new(false);
        let ids = dir.path().join("ids");
        let script = format!(
            "setsid perl -e 'open my $f, q(>), $ARGV[0]; \
               print $f join(q( ), $$, getppid(), getpgrp()); close $f; \
               $x = q(a) x ($ARGV[1]*1024*1024); sleep 30' '{}' 400 & wait; exit 0",
            ids.display()
        );
        // The size comes from an argument: perl folds a constant `x` while it
        // compiles, so a literal 400 MB would be allocated (and the tree
        // killed) before the helper got to write its ids.
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", &script]);
        let started = Instant::now();
        let r = run_capped(
            cmd,
            "grow",
            &cancel,
            None,
            &log,
            64 * 1024 * 1024,
            CapScope::Tree,
        );
        assert!(matches!(r, Err(CappedError::OverMemory)), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(20));
        let ids: Vec<u32> = std::fs::read_to_string(&ids)
            .unwrap()
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        let [perl, shell, group] = ids[..] else {
            panic!("{ids:?}")
        };
        // It really left the group the shell leads.
        assert_ne!(group, shell, "setsid did not move the helper out");
        assert_eq!(group, perl);
        assert!(!still_alive(perl), "the setsid helper outlived the cap");

        // Left behind by a child that exited 0: killed once the child ends.
        let pid_file = dir.path().join("left");
        let script = format!(
            "setsid sleep 30 & echo $! > '{}'; sleep 0.5; exit 0",
            pid_file.display()
        );
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", &script]);
        // The shell outlives one walk of /proc (every 200 ms) after it
        // started the helper: a helper that leaves the group and loses its
        // parent before any walk saw it is not found (`run_capped`'s docs).
        run_capped(cmd, "leave", &cancel, None, &log, 64 << 20, CapScope::Tree).unwrap();
        let left: u32 = wait_for_file(&pid_file, Duration::from_secs(5))
            .trim()
            .parse()
            .unwrap();
        assert!(!still_alive(left), "a helper outlived its finished child");
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
