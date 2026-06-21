//! Headless SilicoLab compute worker.
//!
//! Runs a single engine job staged as `request.json` and writes `outcome.json`,
//! reusing the same `compute-core` engine code the GUI links — so a remote run
//! and a local run agree to convergence tolerance. It understands two commands:
//!
//! - `silicolab-compute exec <request.json> <outcome.json>` — run the staged job
//!   (the same on-disk contract the GUI's hidden `exec` uses).
//! - `silicolab-compute --version` — print the build version, for the deploy pin
//!   check that refuses to run a stale worker.
//!
//! It depends only on `compute-core` (networking off), so the static
//! `x86_64-unknown-linux-musl` build carries no GUI crates and no C toolchain.

use std::path::Path;

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("exec") => {
            let request = args.get(1).context("exec needs a request path")?;
            let outcome = args.get(2).context("exec needs an outcome path")?;
            compute_core::wire::exec(Path::new(request), Path::new(outcome))
        }
        other => bail!(
            "usage: silicolab-compute exec <request.json> <outcome.json> | --version (got {})",
            other.unwrap_or("no command")
        ),
    }
}
