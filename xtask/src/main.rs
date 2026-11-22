//! Build and verification commands used by Marsa contributors.

mod guest;
mod release_signature;
mod upstream;

use std::env;
use std::io;
use std::path::Path;
use std::process::{Command, ExitCode};

#[derive(Clone, Copy)]
enum Action {
    Check,
    Test,
    Deferred,
}

struct Subcommand {
    name: &'static str,
    description: &'static str,
    action: Action,
}

const COMMANDS: &[Subcommand] = &[
    Subcommand {
        name: "check",
        description: "Check formatting, lints, and dependency policy",
        action: Action::Check,
    },
    Subcommand {
        name: "test",
        description: "Run the workspace tests",
        action: Action::Test,
    },
    Subcommand {
        name: "guest",
        description: "Prepare verified guest kernel and filesystem inputs",
        action: Action::Deferred,
    },
    Subcommand {
        name: "fetch-upstream",
        description: "Fetch and verify upstream Apple silicon binaries",
        action: Action::Deferred,
    },
    Subcommand {
        name: "bundle",
        description: "Assemble the macOS app bundle (pending)",
        action: Action::Deferred,
    },
    Subcommand {
        name: "sign",
        description: "Sign and notarize the macOS app (pending)",
        action: Action::Deferred,
    },
    Subcommand {
        name: "verify-signature",
        description: "Verify Developer ID and Hardened Runtime before notarization",
        action: Action::Deferred,
    },
    Subcommand {
        name: "package",
        description: "Build the installer and update archive (pending)",
        action: Action::Deferred,
    },
    Subcommand {
        name: "bench",
        description: "Run the Marsa benchmark suite (pending)",
        action: Action::Deferred,
    },
];

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|argument| argument == "guest")
    {
        return guest::run(&arguments[1..]);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "fetch-upstream")
    {
        return upstream::run(&arguments[1..]);
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "verify-signature")
    {
        return release_signature::run(&arguments[1..]);
    }
    match arguments.as_slice() {
        [] => print_help(),
        [flag] if flag == "--help" || flag == "-h" => print_help(),
        [arg] => {
            let Some(subcommand) = COMMANDS.iter().find(|command| arg == command.name) else {
                eprintln!("Unknown command. Run cargo xtask --help for usage.");
                return ExitCode::from(2);
            };
            if let Err(error) = run(subcommand) {
                eprintln!("cargo xtask {}: {error}", subcommand.name);
                return ExitCode::FAILURE;
            }
        }
        _ => {
            eprintln!("Expected one command. Run cargo xtask --help for usage.");
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}

fn print_help() {
    println!("Usage: cargo xtask <COMMAND>\n\nCommands:");
    for command in COMMANDS {
        println!("  {:<15} {}", command.name, command.description);
    }
}

fn run(command: &Subcommand) -> io::Result<()> {
    match command.action {
        Action::Check => {
            cargo(&["fmt", "--all", "--check"])?;
            cargo(&[
                "clippy",
                "--workspace",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ])?;
            cargo(&[
                "deny",
                "--locked",
                "--config",
                "deny.toml",
                "check",
                "-D",
                "warnings",
            ])
        }
        Action::Test => cargo(&["test", "--workspace", "--locked"]),
        Action::Deferred => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this command is not implemented yet",
        )),
    }
}

fn cargo(args: &[&str]) -> io::Result<()> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let executable = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let display = format!("cargo {}", args.join(" "));
    let status = Command::new(executable)
        .current_dir(workspace)
        .args(args)
        .status()
        .map_err(|error| {
            io::Error::new(error.kind(), format!("could not run {display}: {error}"))
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{display} returned {status}")))
    }
}
