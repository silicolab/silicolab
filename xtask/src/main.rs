use std::{
    collections::BTreeSet,
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("pr-check") => {
            if args.next().is_some() {
                return Err("usage: cargo pr-check".to_owned());
            }
            pr_check()
        }
        Some(command) => Err(format!("unknown xtask command: {command}")),
        None => Err("usage: cargo xtask <command>".to_owned()),
    }
}

fn pr_check() -> Result<(), String> {
    let repo_root = repo_root()?;

    run_cargo(
        &repo_root,
        "Format",
        &["fmt", "--all", "--check"],
        Stdio::inherit(),
    )?;
    run_cargo(
        &repo_root,
        "Clippy",
        &["clippy", "--workspace", "--all-targets", "--all-features"],
        Stdio::inherit(),
    )?;
    assert_worker_pure_rust(&repo_root)?;
    assert_rust_file_sizes(&repo_root)?;
    run_cargo(
        &repo_root,
        "Test",
        &["test", "--workspace", "--all-features"],
        Stdio::inherit(),
    )?;

    println!();
    println!("PR checks passed.");
    Ok(())
}

fn repo_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "could not resolve repository root".to_owned())
}

fn run_cargo(repo_root: &Path, name: &str, args: &[&str], stdout: Stdio) -> Result<(), String> {
    println!();
    println!("==> {name}");

    let status = Command::new("cargo")
        .args(args)
        .current_dir(repo_root)
        .env("CARGO_TERM_COLOR", "always")
        .env("RUSTFLAGS", "-D warnings")
        .env("CARGO_PROFILE_DEV_DEBUG", "line-tables-only")
        .stdout(stdout)
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("failed to run cargo {}: {error}", args.join(" ")))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("{name} failed with {status}."))
    }
}

fn assert_worker_pure_rust(repo_root: &Path) -> Result<(), String> {
    println!();
    println!("==> Worker is pure Rust");

    let output = cargo_output(
        repo_root,
        &[
            "tree",
            "--color",
            "never",
            "-p",
            "silicolab-compute",
            "--target",
            "x86_64-unknown-linux-musl",
            "-e",
            "normal,build",
            "--prefix",
            "none",
        ],
    )?;

    io::copy(&mut output.stderr.as_slice(), &mut io::stderr())
        .map_err(|error| format!("failed to print cargo tree diagnostics: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "worker dependency check failed with {}.",
            output.status
        ));
    }

    let tree = String::from_utf8_lossy(&output.stdout);
    let tree_lines: BTreeSet<_> = tree.lines().collect();
    for line in &tree_lines {
        println!("{line}");
    }

    let native_dependencies: Vec<_> = tree_lines
        .iter()
        .copied()
        .filter(|line| {
            ["cc", "ring", "aws-lc-rs", "aws-lc-sys", "openssl-sys"]
                .iter()
                .any(|name| line.starts_with(&format!("{name} ")))
        })
        .collect();

    if native_dependencies.is_empty() {
        Ok(())
    } else {
        for dependency in native_dependencies {
            eprintln!("{dependency}");
        }
        Err(
            "silicolab-compute pulls a C-toolchain / native build dependency; keep it pure Rust."
                .to_owned(),
        )
    }
}

fn cargo_output(repo_root: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("cargo")
        .args(args)
        .current_dir(repo_root)
        .env("CARGO_TERM_COLOR", "always")
        .env("RUSTFLAGS", "-D warnings")
        .env("CARGO_PROFILE_DEV_DEBUG", "line-tables-only")
        .output()
        .map_err(|error| format!("failed to run cargo {}: {error}", args.join(" ")))
}

fn assert_rust_file_sizes(repo_root: &Path) -> Result<(), String> {
    println!();
    println!("==> Rust file size");

    let output = Command::new("git")
        .args(["ls-files", "--", "*.rs"])
        .current_dir(repo_root)
        .output()
        .map_err(|error| format!("failed to list tracked Rust files: {error}"))?;

    io::copy(&mut output.stderr.as_slice(), &mut io::stderr())
        .map_err(|error| format!("failed to print git diagnostics: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "failed to list tracked Rust files with {}.",
            output.status
        ));
    }

    let files = String::from_utf8(output.stdout)
        .map_err(|error| format!("git produced non-UTF-8 file output: {error}"))?;
    let oversized_files = oversized_rust_files(repo_root, files.lines())?;

    if oversized_files.is_empty() {
        println!("All Rust source files are within the 800 physical-line limit.");
        Ok(())
    } else {
        for (path, line_count) in oversized_files {
            eprintln!("{path} has {line_count} physical lines (limit 800).");
        }
        Err("Split the files above; see the size rule in .rules.".to_owned())
    }
}

fn oversized_rust_files<'a>(
    repo_root: &Path,
    files: impl Iterator<Item = &'a str>,
) -> Result<BTreeSet<(String, usize)>, String> {
    let mut oversized_files = BTreeSet::new();
    for file in files {
        let path = repo_root.join(file);
        let contents = fs::read(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let line_count = physical_line_count(&contents);
        if line_count > 800 {
            oversized_files.insert((file.to_owned(), line_count));
        }
    }
    Ok(oversized_files)
}

fn physical_line_count(contents: &[u8]) -> usize {
    let newline_count = contents.iter().filter(|byte| **byte == b'\n').count();
    if contents.last().is_some_and(|byte| *byte == b'\n') {
        newline_count
    } else {
        newline_count + usize::from(!contents.is_empty())
    }
}
