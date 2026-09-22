//! The `lodger` binary: parse the command line and dispatch.

mod assets;
mod cli;
mod server;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    let result = match cli.command {
        cli::Command::Version => cli::version_line().map(|line| println!("{line}")),
        cli::Command::Serve { listen } => tokio::runtime::Runtime::new()
            .map_err(|e| format!("cannot start the async runtime: {e}"))
            .and_then(|rt| {
                rt.block_on(server::serve(listen))
                    .map_err(|e| format!("cannot serve on {listen}: {e}"))
            }),
    };
    if let Err(e) = result {
        eprintln!("lodger: {e}");
        std::process::exit(1);
    }
}
