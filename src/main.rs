mod app;
mod client;
mod config;
mod editor;
mod file_browser;
mod git_graph;
mod hook;
mod integration;
mod keymap;
mod keys;
mod layout;
mod menu;
mod mouse;
mod onboarding;
mod proto;
mod server;
mod settings;
mod terminal;
mod theme;
mod ui;
mod views;

use std::{env, path::PathBuf};

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};

/// Terminal workspace with persistent PTY panes, a git graph and a file browser.
///
/// Running with no subcommand attaches to the session, starting its background
/// server first if needed. Detach with Ctrl+B d; the shells keep running.
#[derive(Parser)]
#[command(version, about, args_conflicts_with_subcommands = true)]
struct Cli {
    /// Session name.
    #[arg(short, long, global = true, default_value = "default")]
    session: String,

    /// Directory the session starts in (defaults to the current directory).
    path: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the session server in the foreground (started automatically).
    #[command(hide = true)]
    Server {
        #[arg(long)]
        root: PathBuf,
    },
    /// Stop the session server and all its shells.
    Kill,
    /// Report an agent's state to the session (called from agent hooks).
    Hook {
        /// Name shown next to the pane status, e.g. `claude`.
        agent: String,
        #[arg(value_enum)]
        state: proto::ReportState,
    },
    /// Install, remove or check status hooks in coding agents' configs.
    Integration {
        #[arg(value_enum)]
        action: integration::Action,
        /// Which agent (defaults to all supported ones).
        #[arg(value_enum)]
        agent: Option<integration::Agent>,
    },
    /// Find installed coding agents and offer to install status hooks.
    Setup {
        /// Install into every detected agent without asking.
        #[arg(short, long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if !proto::valid_session_name(&cli.session) {
        bail!("session names may only contain letters, digits, '-' and '_'");
    }

    match cli.cmd {
        Some(Cmd::Server { root }) => server::run(&cli.session, root),
        Some(Cmd::Kill) => client::kill(&cli.session),
        Some(Cmd::Hook { agent, state }) => {
            hook::report(&agent, state);
            Ok(())
        }
        Some(Cmd::Integration { action, agent }) => integration::run(action, agent),
        Some(Cmd::Setup { yes }) => onboarding::run(yes),
        None => {
            if onboarding::needed() {
                if let Err(e) = onboarding::run(false) {
                    eprintln!("setup skipped: {e:#}");
                }
            }
            let root = match cli.path {
                Some(p) => p.canonicalize()?,
                None => env::current_dir()?,
            };
            client::attach(&cli.session, &root)
        }
    }
}
