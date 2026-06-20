//! Thin entrypoint: parse args, init logging, dispatch to a subcommand handler.
#![forbid(unsafe_code)]

use clap::Parser;
use lila::cli::Cli;
use lila::{commands, logging};

#[tokio::main]
async fn main() {
    logging::init();
    let cli = Cli::parse();
    let code = commands::dispatch(cli.command).await;
    std::process::exit(code);
}
