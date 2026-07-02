use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
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
    let started_at = Instant::now();

    println!("PR checks");

    run_cargo_step(
        &repo_root,
        1,
        5,
        "fmt",
        &["fmt", "--all", "--check", "--quiet"],
    )?;
    run_cargo_step(
        &repo_root,
        2,
        5,
        "clippy",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--quiet",
        ],
    )?;
    assert_worker_pure_rust(&repo_root, 3, 5)?;
    assert_rust_file_sizes(&repo_root, 4, 5)?;
    run_cargo_step(
        &repo_root,
        5,
        5,
        "test",
        &["test", "--workspace", "--all-features", "--quiet"],
    )?;

    println!(
        "ok PR checks passed ({})",
        format_duration(started_at.elapsed())
    );
    Ok(())
}

fn repo_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "could not resolve repository root".to_owned())
}

fn run_cargo_step(
    repo_root: &Path,
    index: usize,
    total: usize,
    name: &str,
    args: &[&str],
) -> Result<(), String> {
    let started_at = Instant::now();
    print_step(index, total, name);
    let output = cargo_output(repo_root, args)?;

    if !output.status.success() {
        print_command_failure("cargo", args, &output);
        return Err(format!("{name} failed with {}.", output.status));
    }

    println!("ok ({})", format_duration(started_at.elapsed()));
    Ok(())
}

fn print_step(index: usize, total: usize, name: &str) {
    print!("[{index}/{total}] {name} ... ");
}

fn assert_worker_pure_rust(repo_root: &Path, index: usize, total: usize) -> Result<(), String> {
    let started_at = Instant::now();
    print_step(index, total, "worker dependency purity");

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

    if !output.status.success() {
        print_command_failure(
            "cargo",
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
            &output,
        );
        return Err(format!(
            "worker dependency check failed with {}.",
            output.status
        ));
    }

    let tree = String::from_utf8_lossy(&output.stdout);
    let tree_lines: BTreeSet<_> = tree.lines().collect();

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
        println!(
            "ok ({} deps, {})",
            tree_lines.len(),
            format_duration(started_at.elapsed())
        );
        Ok(())
    } else {
        println!("failed");
        for dependency in native_dependencies {
            eprintln!("{dependency}");
        }
        Err(
            "silicolab-compute pulls a C-toolchain / native build dependency; keep it pure Rust."
                .to_owned(),
        )
    }
}

fn cargo_output(repo_root: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new("cargo")
        .args(args)
        .current_dir(repo_root)
        .env("CARGO_TERM_COLOR", "always")
        .env("RUSTFLAGS", "-D warnings")
        .env("CARGO_PROFILE_DEV_DEBUG", "line-tables-only")
        .output()
        .map_err(|error| format!("failed to run cargo {}: {error}", args.join(" ")))
}

fn assert_rust_file_sizes(repo_root: &Path, index: usize, total: usize) -> Result<(), String> {
    let started_at = Instant::now();
    print_step(index, total, "rust file size");

    let output = Command::new("git")
        .args(["ls-files", "--", "*.rs"])
        .current_dir(repo_root)
        .output()
        .map_err(|error| format!("failed to list tracked Rust files: {error}"))?;

    if !output.status.success() {
        print_command_failure("git", &["ls-files", "--", "*.rs"], &output);
        return Err(format!(
            "failed to list tracked Rust files with {}.",
            output.status
        ));
    }

    let files = String::from_utf8(output.stdout)
        .map_err(|error| format!("git produced non-UTF-8 file output: {error}"))?;
    let rust_files: Vec<_> = files.lines().collect();
    let oversized_files = oversized_rust_files(repo_root, rust_files.iter().copied())?;

    if oversized_files.is_empty() {
        println!(
            "ok ({} files, {})",
            rust_files.len(),
            format_duration(started_at.elapsed())
        );
        Ok(())
    } else {
        println!("failed");
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

fn print_command_failure(program: &str, args: &[&str], output: &Output) {
    println!("failed");
    eprintln!("command: {program} {}", args.join(" "));
    print_stream("stdout", &output.stdout);
    print_stream("stderr", &output.stderr);
}

fn print_stream(name: &str, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    if text.trim().is_empty() {
        return;
    }

    eprintln!("--- {name} ---");
    eprint!("{text}");
    if !text.ends_with('\n') {
        eprintln!();
    }
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs_f64();
    if seconds < 10.0 {
        format!("{seconds:.1}s")
    } else {
        format!("{seconds:.0}s")
    }
}
