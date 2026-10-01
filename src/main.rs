mod agent;
mod cli;
mod curator;
mod curmudgeon;
mod inference;
mod knowledge;
mod maintenance;
mod scholar;
mod scout;
mod storage;
mod web;

fn main() {
    if let Err(error) = cli::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
