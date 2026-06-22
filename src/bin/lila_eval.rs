//! `lila-eval` — the behavior eval runner. Boots the compiled `lila` binary against the scenario
//! suite at production parity and reports scores + the token breakdown. Thin entry: parse args,
//! run, propagate the exit code.

use clap::Parser;
use lila::eval::run::{self, Args};

#[tokio::main]
async fn main() {
    let args = Args::parse();
    std::process::exit(run::main(args).await);
}
