mod agent;
mod cli;
mod curator;
mod curmudgeon;
mod diagnostics;
mod entities;
mod inference;
mod knowledge;
mod maintenance;
mod research;
mod scholar;
mod scout;
mod semantic;
mod storage;
mod web;

fn main() {
    if let Err(error) = cli::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
