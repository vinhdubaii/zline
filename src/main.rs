//! Z-Line — a terminal Zalo client built on the ctdozlite protocol core.

mod cli;
mod cmd;
mod ctx;
mod fmt;
mod paths;
mod threads;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("✗ {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let g = &cli.global;
    match cli.cmd {
        Command::Login { force } => cmd::auth::login(g, force).await,
        Command::Logout { purge } => cmd::auth::logout(g, purge),
        Command::Whoami { offline } => cmd::auth::whoami(g, offline).await,
        Command::Ls {
            groups,
            friends,
            refresh,
            limit,
        } => cmd::ls::run(g, groups, friends, refresh, limit).await,
        Command::Read { thread, count } => cmd::read::run(g, &thread, count),
        Command::Send { thread, text } => cmd::send::run(g, &thread, text).await,
        Command::Sync { thread } => cmd::sync::run(g, thread).await,
    }
}
