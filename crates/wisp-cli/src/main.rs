//! `wisp` command-line interface (§16).
//!
//! With `--json`, every command writes exactly one JSON object to stdout with fields `ok`,
//! `command`, `data`, and on failure `error: {code, message, details}`. Human-readable output
//! goes to stderr. Exit codes are stable (§16).

mod cli;
mod doc_paths;
mod help_text;
mod run;

use clap::Parser;
use wisp_model::{ExitCode, WispError};

fn main() {
    let args = cli::Cli::parse();
    let json = args.json;
    let quiet = args.quiet;
    let command = args.command_name();
    match run::run(args) {
        Ok(output) => {
            if json {
                let envelope =
                    serde_json::json!({ "ok": true, "command": command, "data": output.data });
                println!("{}", serde_json::to_string(&envelope).unwrap_or_default());
            } else if !quiet {
                match output.human {
                    Some(h) => println!("{h}"),
                    None => println!(
                        "{}",
                        serde_json::to_string_pretty(&output.data).unwrap_or_default()
                    ),
                }
            }
            std::process::exit(output.exit.code());
        }
        Err(e) => {
            let code = e.code;
            if json {
                let envelope = serde_json::json!({ "ok": false, "command": command, "error": e });
                println!("{}", serde_json::to_string(&envelope).unwrap_or_default());
            } else {
                eprintln!("wisp: {}", e.message);
                if !e.details.is_null() && !quiet {
                    eprintln!(
                        "{}",
                        serde_json::to_string_pretty(&e.details).unwrap_or_default()
                    );
                }
            }
            std::process::exit(code.code());
        }
    }
}

/// What a command produced.
pub struct Output {
    pub data: serde_json::Value,
    pub human: Option<String>,
    pub exit: ExitCode,
}

impl Output {
    pub fn json(data: serde_json::Value) -> Self {
        Output {
            data,
            human: None,
            exit: ExitCode::Success,
        }
    }
    pub fn text(data: serde_json::Value, human: impl Into<String>) -> Self {
        Output {
            data,
            human: Some(human.into()),
            exit: ExitCode::Success,
        }
    }
    pub fn with_exit(mut self, e: ExitCode) -> Self {
        self.exit = e;
        self
    }
}

pub type CliResult = Result<Output, WispError>;
