//! Curust as a console program: command line slicing (`curust-cli slice-models ...`), the
//! CuraEngine-compatible `slice` command, and the interface (`curust-cli gui`) with its log in the
//! console.

fn main() {
    curust::run(std::env::args().collect());
}
