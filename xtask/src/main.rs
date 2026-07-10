use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};

mod dev_worker;

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
        Some("build-dev-worker") => {
            if args.next().is_some() {
                return Err("usage: cargo xtask build-dev-worker".to_owned());
            }
            dev_worker::build().map(|_| ())
        }
        Some("remote-dev") => {
            let app_args = match args.next() {
                Some(separator) if separator == "--" => args.collect(),
                Some(_) => {
                    return Err("usage: cargo xtask remote-dev [-- <app args>]".to_owned());
                }
                None => Vec::new(),
            };
            dev_worker::run(&app_args)
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
        8,
        "fmt",
        &["fmt", "--all", "--check", "--quiet"],
        Warnings::Allow,
    )?;
    run_cargo_step(
        &repo_root,
        2,
        8,
        "clippy",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--quiet",
        ],
        Warnings::Deny,
    )?;
    run_cargo_step(
        &repo_root,
        3,
        8,
        "default-feature clippy",
        &["clippy", "--workspace", "--all-targets", "--quiet"],
        Warnings::Deny,
    )?;
    assert_worker_pure_rust(&repo_root, 4, 8)?;
    build_dev_worker_step(5, 8)?;
    assert_rust_file_sizes(&repo_root, 6, 8)?;
    run_cargo_step(
        &repo_root,
        7,
        8,
        "test",
        &["test", "--workspace", "--all-features", "--quiet"],
        Warnings::Allow,
    )?;
    run_cargo_step(
        &repo_root,
        8,
        8,
        "production worker deploy tests",
        &[
            "test",
            "--package",
            "compute-core",
            "--lib",
            "engines::remote::artifact",
            "--quiet",
        ],
        Warnings::Allow,
    )?;

    println!(
        "ok PR checks passed ({})",
        format_duration(started_at.elapsed())
    );
    Ok(())
}

fn build_dev_worker_step(index: usize, total: usize) -> Result<(), String> {
    let started_at = Instant::now();
    print_step(index, total, "development worker build");
    println!();
    let worker = dev_worker::build()?;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    dev_worker::smoke_test_version(&worker)?;
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    let _ = worker;
    println!("ok ({})", format_duration(started_at.elapsed()));
    Ok(())
}

fn repo_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "could not resolve repository root".to_owned())
}

/// Mirrors CI: only the clippy job denies warnings, so a new rustc lint fails
/// lint checking rather than masquerading as a test failure.
#[derive(Clone, Copy)]
enum Warnings {
    Deny,
    Allow,
}

fn run_cargo_step(
    repo_root: &Path,
    index: usize,
    total: usize,
    name: &str,
    args: &[&str],
    warnings: Warnings,
) -> Result<(), String> {
    let started_at = Instant::now();
    print_step(index, total, name);
    let output = cargo_output(repo_root, args, warnings)?;

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
        Warnings::Allow,
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

fn cargo_output(repo_root: &Path, args: &[&str], warnings: Warnings) -> Result<Output, String> {
    let mut command = Command::new("cargo");
    command
        .args(args)
        .current_dir(repo_root)
        .env("CARGO_TERM_COLOR", "always")
        .env("CARGO_PROFILE_DEV_DEBUG", "line-tables-only");
    if let Warnings::Deny = warnings {
        command.env("RUSTFLAGS", "-D warnings");
    }
    command
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
