// Release builds are GUI-subsystem executables (no console window).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let started = std::time::Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(paper_mirror::app::run(args, started));
}
