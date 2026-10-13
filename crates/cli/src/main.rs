//! `main.cpp`: the CuraEngine command line executable.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Windows gives the main thread only 1 MiB of stack; slice on a thread with a large one.
    let worker = std::thread::Builder::new().name("engine".into()).stack_size(64 << 20).spawn(move || curust_engine::application::run(args));
    match worker {
        Ok(h) => {
            if h.join().is_err() {
                std::process::exit(101);
            }
        }
        Err(e) => {
            eprintln!("Unable to start: {}", e);
            std::process::exit(1);
        }
    }
}
