//! Windows integration: process metrics, dialogs and shell services. Everything that
//! calls Win32 directly lives under this module so the rest of the code stays portable
//! in shape, if not in practice.

pub mod memory;
