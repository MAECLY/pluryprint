//! `pluryprint-bridge` — the Chrome Native Messaging host that receives
//! documents from the Pluryfact extension (FR-120, ADR-0016).
//!
//! The browser starts it with the calling extension's origin as the first
//! argument (and, on Windows, `--parent-window=<hwnd>` as the second) and
//! talks to it over stdin/stdout in length-prefixed JSON. Everything lives in
//! [`pluryprint_lib::bridge`]; this file only hands it the process' argv and
//! exits with its code.
//!
//! It is a console program on every OS (no `windows_subsystem` attribute):
//! Native Messaging needs real standard handles, and Chrome starts the host
//! without a visible console window. stdout carries protocol frames only;
//! logs go to the app's log file.

fn main() {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let code = pluryprint_lib::bridge::run_stdio(argv);
    std::process::exit(code);
}
