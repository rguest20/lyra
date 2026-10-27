mod agent;
mod cli;
mod curator;
mod curmudgeon;
mod diagnostics;
mod entities;
mod errors;
mod inference;
mod knowledge;
mod maintenance;
mod research;
mod research_intake;
mod scholar;
mod scout;
mod semantic;
mod storage;
#[cfg(test)]
mod test_support;
mod web;

fn main() {
    if let Err(error) = cli::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
