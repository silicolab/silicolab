#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use std::env;

use anyhow::Result;
use silicolab::{domain::Structure, frontend};

fn main() -> Result<()> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        return frontend::run(Structure::empty(), None);
    }

    if matches!(args[0].as_str(), "help" | "--help" | "-h") {
        print!("{}", frontend::cli_help_text());
        return Ok(());
    }

    let request = frontend::parse_cli_script_request(&args)?;
    if let Some(request) = request {
        let result = frontend::run_cli_script(&request)?;
        for line in &result.stdout_lines {
            println!("{line}");
        }
        return Ok(());
    }

    frontend::run(Structure::empty(), None)
}
