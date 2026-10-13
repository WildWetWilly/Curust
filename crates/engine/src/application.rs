//! `Application.h`: global state of the engine (the current slice and the worker thread count).
//!
//! CuraEngine accesses the current slice through a global singleton in hundreds of places; this
//! port mirrors that with a process-global slot. The slice is set up single-threaded and then only
//! read concurrently (apart from documented single-threaded mutation points).

use crate::communication::Communication;
use crate::slice::Slice;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

struct GlobalSlice(UnsafeCell<Option<Box<Slice>>>);
// SAFETY: see module documentation.
unsafe impl Sync for GlobalSlice {}

static CURRENT_SLICE: GlobalSlice = GlobalSlice(UnsafeCell::new(None));
static THREAD_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Replace the current slice.
pub fn set_current_slice(slice: Box<Slice>) {
    unsafe {
        *CURRENT_SLICE.0.get() = Some(slice);
    }
}

/// Drop the current slice.
pub fn clear_current_slice() {
    unsafe {
        *CURRENT_SLICE.0.get() = None;
    }
}

/// `Application::getInstance().current_slice_`.
pub fn current_slice() -> &'static Slice {
    try_current_slice().expect("no current slice")
}

pub fn try_current_slice() -> Option<&'static Slice> {
    unsafe { (*CURRENT_SLICE.0.get()).as_deref() }
}

/// Mutable access to the current slice. Only to be used in single-threaded phases.
#[allow(clippy::mut_from_ref)]
pub fn current_slice_mut() -> &'static mut Slice {
    unsafe { (*CURRENT_SLICE.0.get()).as_deref_mut().expect("no current slice") }
}

/// `Application::startThreadPool(nworkers)`: total number of threads to use (0 = all cores).
pub fn start_thread_pool(nworkers: usize) {
    let n = if nworkers == 0 {
        if THREAD_COUNT.load(Ordering::Relaxed) != 0 {
            return;
        }
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    } else {
        nworkers
    };
    THREAD_COUNT.store(n.max(1), Ordering::Relaxed);
}

/// Number of threads to use for parallel loops.
pub fn thread_count() -> usize {
    let n = THREAD_COUNT.load(Ordering::Relaxed);
    if n == 0 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    } else {
        n
    }
}

struct GlobalCommunication(UnsafeCell<Option<Box<dyn Communication>>>);
// SAFETY: the communication object is only used from the main thread.
unsafe impl Sync for GlobalCommunication {}

static COMMUNICATION: GlobalCommunication = GlobalCommunication(UnsafeCell::new(None));

/// `Application::getInstance().communication_ = ...`.
pub fn set_communication(c: Box<dyn Communication>) {
    unsafe {
        *COMMUNICATION.0.get() = Some(c);
    }
}

/// `Application::getInstance().communication_`.
#[allow(clippy::mut_from_ref)]
pub fn communication() -> &'static mut dyn Communication {
    unsafe { (*COMMUNICATION.0.get()).as_deref_mut().expect("no communication") }
}

pub fn has_communication() -> bool {
    unsafe { (*COMMUNICATION.0.get()).is_some() }
}

static ARGV: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// `Application::printCall()`.
pub fn print_call() {
    if let Some(argv) = ARGV.get() {
        crate::log_error!("Command called: {}", argv.first().map(|s| s.as_str()).unwrap_or(""));
    }
}

/// `Application::printHelp()`.
pub fn print_help() {
    print!("\n");
    print!("usage:\n");
    print!("CuraEngine help\n");
    print!("\tShow this help message\n");
    print!("\n");
    print!("CuraEngine slice [-v] [-p] [-j <settings.json>] [-s <settingkey>=<value>] [-g] [-e<extruder_nr>] [-o <output.gcode>] [-l <model.stl>] [--next]\n");
    print!("  -v\n\tIncrease the verbose level (show log messages).\n");
    print!("  -m<thread_count>\n\tSet the desired number of threads.\n");
    print!("  -p\n\tLog progress information.\n");
    print!("  -d Add definition search paths seperated by a `:` (Unix) or `;` (Windows)\n");
    print!("  -j\n\tLoad settings.def.json file to register all settings and their defaults.\n");
    print!("  -r\n\tLoad a json file containing resolved setting values.\n");
    print!("  -s <setting>=<value>\n\tSet a setting to a value for the last supplied object, \n\textruder train, or general settings.\n");
    print!("  -l <model_file>\n\tLoad an STL model. \n");
    print!("  -g\n\tSwitch setting focus to the current mesh group only.\n\tUsed for one-at-a-time printing.\n");
    print!("  -e<extruder_nr>\n\tSwitch setting focus to the extruder train with the given number.\n");
    print!("  --next\n\tGenerate gcode for the previously supplied mesh group and append that to \n\tthe gcode of further models for one-at-a-time printing.\n");
    print!("  -o <output_file>\n\tSpecify a file to which to write the generated gcode.\n");
    print!("\n");
    print!("The settings are appended to the last supplied object:\n");
    print!("CuraEngine slice [general settings] \n\t-g [current group settings] \n\t-e0 [extruder train 0 settings] \n\t-l obj_inheriting_from_last_extruder_train.stl [object settings] \n\t--next [next group settings]\n\t... etc.\n");
    print!("\n");
    print!("In order to load machine definitions from custom locations, you need to create the environment variable CURA_ENGINE_SEARCH_PATH, which should contain all search paths delimited by a (semi-)colon.\n");
    print!("\n");
}

fn print_header() {
    print!("\n");
    print!("Cura_SteamEngine version {}\n", crate::settings::settings::CURA_ENGINE_VERSION);
}

fn print_license() {
    print!("\n");
    print!("Copyright (C) 2024 Ultimaker\n");
    print!("\n");
    print!("This program is free software: you can redistribute it and/or modify\n");
    print!("it under the terms of the GNU Affero General Public License as published by\n");
    print!("the Free Software Foundation, either version 3 of the License, or\n");
    print!("(at your option) any later version.\n");
    print!("\n");
    print!("This program is distributed in the hope that it will be useful,\n");
    print!("but WITHOUT ANY WARRANTY; without even the implied warranty of\n");
    print!("MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the\n");
    print!("GNU Affero General Public License for more details.\n");
    print!("\n");
    print!("You should have received a copy of the GNU Affero General Public License\n");
    print!("along with this program.  If not, see <http://www.gnu.org/licenses/>.\n");
}

/// `Application::run(argc, argv)`.
pub fn run(argv: Vec<String>) {
    let _ = ARGV.set(argv.clone());
    if let Ok(level) = std::env::var("CURAENGINE_LOG_LEVEL") {
        match level.as_str() {
            "debug" | "trace" => crate::logging::set_level(crate::logging::LEVEL_DEBUG),
            "info" => crate::logging::set_level(crate::logging::LEVEL_INFO),
            "warn" | "warning" => crate::logging::set_level(crate::logging::LEVEL_WARN),
            "err" | "error" | "critical" => crate::logging::set_level(crate::logging::LEVEL_ERROR),
            "off" => crate::logging::set_level(crate::logging::LEVEL_OFF),
            _ => {}
        }
    }
    print_header();
    print_license();

    if argv.len() < 2 {
        print_help();
        std::process::exit(1);
    }

    let command = argv[1].to_ascii_lowercase();
    if command == "engine-job" {
        if argv.len() < 4 {
            crate::log_error!("usage: engine-job <job file> <result file>");
            std::process::exit(1);
        }
        if let Err(e) = crate::communication::job::run_job(&argv[2], &argv[3]) {
            crate::log_error!("{}", e);
            std::process::exit(1);
        }
        return;
    }
    if command == "slice" {
        set_communication(Box::new(crate::communication::command_line::CommandLine::new(argv.clone())));
    } else if command == "help" {
        print_help();
    } else {
        crate::log_error!("Unknown command: {}", argv[1]);
        print_call();
        print_help();
        std::process::exit(1);
    }

    if !has_communication() {
        // No communication channel is open any more, so either:
        //- communication failed to connect, or
        //- the engine was called with an unknown command or a command that doesn't connect (like "help").
        // In either case, we don't want to slice.
        std::process::exit(0);
    }
    start_thread_pool(0); // Start the thread pool
    while communication().has_slice() {
        communication().slice_next();
    }
}
