//! Worker kind `render-pdf`: an HTML page → PDF through the system web view
//! (FR-055, D15), driven by [`crate::render`].
//!
//! ```text
//! pluryprint --worker render-pdf --page <stage>/page.html --base <doc folder>
//!            --out <work>/out.pdf --data <work>/webview [--paper-in 8.27x11.69]
//! ```
//!
//! The arguments are a [`RenderRequest`]. Exit codes follow [`super`]: `0`
//! with the PDF written, `1` with the reason on stderr (the engine quotes its
//! tail), `2` for arguments it does not understand. The engine bounds the run
//! with the conversion timeout and the job's cancel flag and kills the whole
//! process tree when either fires, web view helpers included.
//!
//! Before it reads the page it applies the memory cap its parent passed
//! ([`crate::engine::convert::process::apply_memory_cap`]), as the SVG worker
//! does: on Windows it waits until it is inside the parent's Job Object,
//! whose limit covers the web view's processes too.
//!
//! Linux is the exception: the worker does not lower its own `RLIMIT_DATA`.
//! WebKitGTK's JavaScript engine reserves several gigabytes of writable
//! address space when it starts, scripts off or not, and `RLIMIT_DATA`
//! counts reservations, not memory in use: under the 2 GB render cap the
//! worker aborted before it loaded anything (measured: it needs between 4
//! and 8 GB of it for a one-line page). There the cap is the parent's
//! watchdog over the worker's whole process tree: its group and every
//! descendant, by parent links, so a web process started in a session of
//! its own (WebKit's `bwrap` sandbox starts one) is still counted and
//! killed ([`crate::engine::convert::process::run_capped`],
//! `CapScope::Tree`). Two things on the worker's side back it up:
//!
//! - The worker makes itself a *child subreaper*: a helper whose parent
//!   exits is handed to the worker rather than to `init`, so it stays a
//!   descendant the watchdog can find for as long as the worker lives.
//! - WebKit's own memory limit for the web process is set to the same cap
//!   ([`crate::render`]'s Linux renderer), so a page that outgrows it ends
//!   as out of memory even when no parent is watching (a crashed app).

use std::ffi::OsString;

use crate::render::{self, RenderRequest};

/// Run the render worker with its own arguments; returns the exit code.
pub fn run(args: &[OsString]) -> i32 {
    let request = match RenderRequest::parse(args) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("pluryprint worker render-pdf: {message}");
            return super::EXIT_USAGE;
        }
    };
    // ADR-0014: the memory cap is in force before the page is read (on
    // Linux the parent's watchdog over the worker's tree; module docs).
    #[cfg(not(target_os = "linux"))]
    if let Err(e) = crate::engine::convert::process::apply_memory_cap() {
        eprintln!("pluryprint worker render-pdf: could not apply the memory limit: {e}");
        return super::EXIT_FAILED;
    }
    #[cfg(target_os = "linux")]
    become_subreaper();
    match render::render(&request) {
        Ok(()) => super::EXIT_OK,
        Err(message) => {
            eprintln!("{}{message}", render::WORKER_ERROR_PREFIX);
            super::EXIT_FAILED
        }
    }
}

/// Make the worker the child subreaper of everything it starts (module
/// docs): an orphaned helper is re-parented to the worker, not to `init`,
/// and so stays inside the tree the parent's watchdog walks. Best effort:
/// on failure only a helper that double-forks escapes the walk, which is
/// how things were before.
#[cfg(target_os = "linux")]
fn become_subreaper() {
    // SAFETY: prctl with integer arguments only.
    let set = unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == 0;
    if !set {
        eprintln!(
            "pluryprint worker render-pdf: could not become a subreaper: {}",
            std::io::Error::last_os_error()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfr_worker_render_refuses_bad_arguments_with_the_usage_code() {
        assert_eq!(run(&[]), super::super::EXIT_USAGE);
        assert_eq!(
            run(&["--page".into(), "a.html".into()]),
            super::super::EXIT_USAGE
        );
    }
}
