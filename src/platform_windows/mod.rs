//! Windows integration: process metrics, dialogs and shell services. Everything that
//! calls Win32 directly lives under this module.

pub mod clipboard;
pub mod dpapi;
pub mod memory;
pub mod random;
pub mod shell;
