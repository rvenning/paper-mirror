//! Paper Mirror: a lightweight Windows PDF and image viewer inspired by macOS Preview.

pub mod app;
pub mod core;
#[doc(hidden)]
pub mod fixtures;
pub mod images;
pub mod installer;
pub mod pdf;
pub mod platform_windows;
pub mod storage;

pub mod ui {
    slint::include_modules!();
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
