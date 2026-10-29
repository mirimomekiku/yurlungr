//! First-run setup: find the coding agents on this machine and offer to install
//! yurlungr's status hooks into them. Re-run any time with `yurlungr setup`.

use std::{
    fs,
    io::{self, IsTerminal, Write},
    path::PathBuf,
};

use anyhow::Result;

use crate::{
    brand,
    integration::{Action, Agent, ALL},
    proto,
};

#[derive(Debug, PartialEq, Eq)]
enum Choice {
    All,
    Nothing,
    Each,
}

fn parse_choice(answer: &str) -> Option<Choice> {
    match answer.trim().to_lowercase().as_str() {
        "" | "y" | "yes" | "a" | "all" => Some(Choice::All),
        "n" | "no" | "s" | "skip" => Some(Choice::Nothing),
        "c" | "choose" | "e" | "each" => Some(Choice::Each),
        _ => None,
    }
}

fn marker() -> Result<PathBuf> {
    Ok(proto::data_dir()?.join("onboarded"))
}

/// True on the very first interactive run.
pub fn needed() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal() && marker().is_ok_and(|m| !m.exists())
}

fn read_line() -> String {
    let mut line = String::new();
    // EOF reads as an empty answer; callers treat that as "no" where it matters.
    let _ = io::stdin().read_line(&mut line);
    line
}

fn ask(question: &str) -> bool {
    print!("{question} [Y/n] ");
    let _ = io::stdout().flush();
    let answer = read_line();
    !matches!(answer.trim().to_lowercase().as_str(), "n" | "no")
}

pub fn run(yes: bool) -> Result<()> {
    brand::show(brand::Color::from_env(io::stdout().is_terminal()));
    println!("\n  First-run setup\n");
    println!(
        "Hooks let each pane show exactly what its agent is doing (working / blocked / idle)."
    );
    println!(
        "They are added to the agent's own config; a one-time .yurlungr.bak backup is kept.\n"
    );

    let mut todo: Vec<Agent> = Vec::new();
    for agent in ALL {
        let found = agent.detected();
        let installed = agent.is_installed().unwrap_or(false);
        let status = match (installed, found) {
            (true, _) => "hooks already installed",
            (false, true) => "found",
            (false, false) => "not found",
        };
        println!("  {:<15} {status}", agent.label());
        if found && !installed {
            todo.push(agent);
        }
    }
    println!();

    if todo.is_empty() {
        println!("Nothing to install. Use `yurlungr integration install <agent>` to force one.");
    } else {
        let names: Vec<&str> = todo.iter().map(|a| a.label()).collect();
        let choice = if yes {
            Choice::All
        } else {
            loop {
                print!("Install hooks for {}? [Y]es / [n]o / [c]hoose each: ", names.join(", "));
                let _ = io::stdout().flush();
                let answer = read_line();
                if answer.is_empty() {
                    break Choice::Nothing; // EOF
                }
                if let Some(c) = parse_choice(&answer) {
                    break c;
                }
            }
        };

        for agent in todo {
            let go = match choice {
                Choice::All => true,
                Choice::Nothing => false,
                Choice::Each => ask(&format!("Install for {}?", agent.label())),
            };
            if !go {
                continue;
            }
            match agent.apply(Action::Install) {
                Ok(msg) => println!("  {:<15} {msg}", agent.label()),
                Err(e) => eprintln!("  {:<15} error: {e:#}", agent.label()),
            }
        }
    }

    println!("\nAgents without a hook system are still tracked by watching their screen.");
    println!("Restart any running agent so it picks up the new hooks.\n");

    fs::write(marker()?, "")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_parse() {
        assert_eq!(parse_choice(""), Some(Choice::All));
        assert_eq!(parse_choice(" Y\n"), Some(Choice::All));
        assert_eq!(parse_choice("no"), Some(Choice::Nothing));
        assert_eq!(parse_choice("c"), Some(Choice::Each));
        assert_eq!(parse_choice("maybe"), None);
    }
}
