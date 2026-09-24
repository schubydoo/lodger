//! The `lodger` binary: parse the command line and dispatch.

mod accounts;
mod actions;
mod admin;
mod api;
mod assets;
mod audit;
mod auth;
mod cli;
mod client_ip;
mod config;
mod console;
mod db;
mod install;
mod passwords;
mod security;
mod server;
mod setup;
mod stats;
mod throttle;
mod tickets;
mod ws;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    let result = match cli.command {
        cli::Command::Version => cli::version_line().map(|line| println!("{line}")),
        cli::Command::Install => install::run_install().map(|line| println!("{line}")),
        cli::Command::Uninstall { purge } => {
            install::run_uninstall(purge).map(|line| println!("{line}"))
        }
        cli::Command::Admin {
            action,
            config,
            state_dir,
        } => admin::run(
            action,
            config::Overrides {
                config,
                state_dir,
                ..Default::default()
            },
        )
        .map(|line| println!("{line}")),
        cli::Command::Serve {
            config,
            listen,
            uri,
            state_dir,
        } => config::Config::load(
            config::Overrides {
                config,
                listen,
                uri,
                state_dir,
            },
            std::env::var("STATE_DIRECTORY").ok().as_deref(),
        )
        .and_then(|config| {
            tokio::runtime::Runtime::new()
                .map_err(|e| format!("cannot start the async runtime: {e}"))
                .and_then(|rt| rt.block_on(server::serve(config)))
        }),
    };
    if let Err(e) = result {
        eprintln!("lodger: {e}");
        std::process::exit(1);
    }
}
