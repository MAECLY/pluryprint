//! Small cross-cutting helpers: the IPC error type shared by every command
//! and CLI path, filesystem path resolution, and seeding the WebView2
//! profiles so they skip Chromium's blank-password logon probe.

pub mod appimage;
pub mod chromium;
pub mod error;
pub mod legacy;
pub mod paths;
pub mod reasons;
