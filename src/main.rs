#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use std::env;
use std::path::Path;

use anyhow::{Context, Result};
use silicolab::{domain::Structure, frontend};

fn main() -> Result<()> {
    let args = env::args().skip(1).collect::<Vec<_>>();

    // Hidden engine-exec entry: read a staged request, run the engine, write the
    // outcome. Handled before the script path and kept out of the console grammar,
    // so it never surfaces to users or the command catalog.
    if args.first().map(String::as_str) == Some("exec") {
        let request = args.get(1).context("exec needs a request path")?;
        let outcome = args.get(2).context("exec needs an outcome path")?;
        return silicolab::wire::exec(Path::new(request), Path::new(outcome));
    }

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
