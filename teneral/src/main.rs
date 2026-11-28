//! Teneral: a daemon that answers a request from an identified client
//! on a known interface with the content configured for that client.
//!
//! `teneral serve` is the daemon. Every other subcommand is a client of
//! its control socket, there to be pointed at a running daemon by a
//! person debugging one; none of them holds a rule the daemon should
//! hold, so each is one call and a printing of the answer.

mod cli;
mod client;
mod dhcp;
mod serve;
mod sweep;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();

    // Standard error, with no logging framework. A systemd unit
    // captures standard error into the journal, which supplies the
    // timestamps and the filtering, and choosing a framework belongs
    // with how the daemon is packaged rather than with whether it can
    // be run.
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("teneral: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Everything fallible, so that `main` has one place an error is
/// printed and one exit code for it.
fn run(cli: cli::Cli) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("starting the runtime: {error}"))?;

    match cli.command {
        cli::Command::Serve(args) => serve::run(runtime, &args),
        cli::Command::Client(command) => runtime.block_on(client::run(command)),
    }
}
