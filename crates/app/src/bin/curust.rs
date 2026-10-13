//! The Curust application. Without arguments it opens the graphical interface; it accepts the
//! same commands as `curust-cli`. On Windows it is a windowed program (no console window); a
//! command run from a console prints to that console.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(windows)]
    attach_console(&args);
    curust::run(args);
}

/// Commands other than the interface (and the engine the interface starts) write to the console
/// they were started from.
#[cfg(windows)]
fn attach_console(args: &[String]) {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(pid: u32) -> i32;
    }
    let Some(cmd) = args.get(1) else { return };
    let gui = cmd == "gui" || cmd == "engine-job" || cmd.starts_with("--") || std::path::Path::new(cmd).is_file();
    if !gui {
        // SAFETY: ATTACH_PARENT_PROCESS; fails harmlessly when there is no parent console.
        unsafe {
            AttachConsole(u32::MAX);
        }
    }
}
