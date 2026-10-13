//! Minimal spdlog-like logging (no external dependencies).

use std::sync::atomic::{AtomicU8, Ordering};

pub const LEVEL_DEBUG: u8 = 0;
pub const LEVEL_INFO: u8 = 1;
pub const LEVEL_WARN: u8 = 2;
pub const LEVEL_ERROR: u8 = 3;
pub const LEVEL_OFF: u8 = 4;

static LEVEL: AtomicU8 = AtomicU8::new(LEVEL_INFO);

pub fn set_level(level: u8) {
    LEVEL.store(level, Ordering::Relaxed);
}

pub fn level() -> u8 {
    LEVEL.load(Ordering::Relaxed)
}

pub fn enabled(level: u8) -> bool {
    level >= LEVEL.load(Ordering::Relaxed)
}

pub fn write(level: u8, msg: std::fmt::Arguments) {
    if !enabled(level) {
        return;
    }
    let name = match level {
        LEVEL_DEBUG => "debug",
        LEVEL_INFO => "info",
        LEVEL_WARN => "warning",
        _ => "error",
    };
    eprintln!("[{}] {}", name, msg);
}

#[macro_export]
macro_rules! log_debug { ($($arg:tt)*) => { $crate::logging::write($crate::logging::LEVEL_DEBUG, format_args!($($arg)*)) } }
#[macro_export]
macro_rules! log_info { ($($arg:tt)*) => { $crate::logging::write($crate::logging::LEVEL_INFO, format_args!($($arg)*)) } }
#[macro_export]
macro_rules! log_warn { ($($arg:tt)*) => { $crate::logging::write($crate::logging::LEVEL_WARN, format_args!($($arg)*)) } }
#[macro_export]
macro_rules! log_error { ($($arg:tt)*) => { $crate::logging::write($crate::logging::LEVEL_ERROR, format_args!($($arg)*)) } }
