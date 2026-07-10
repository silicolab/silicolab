use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

use super::{print_command_failure, repo_root};

const TARGET: &str = "x86_64-unknown-linux-musl";
const ARTIFACT_ENV: &str = "SILICOLAB_DEV_WORKER";

pub fn run(app_args: &[String]) -> Result<(), String> {
    let repo_root = repo_root()?;
    let built_worker = build()?;
    let worker = env::var_os(ARTIFACT_ENV)
        .map(PathBuf::from)
        .unwrap_or(built_worker);

    println!("Launching SilicoLab with {}", worker.display());
    let status = Command::new("cargo")
        .args([
            "run",
            "--package",
            "silicolab",
            "--features",
            "dev-worker",
            "--",
        ])
        .args(app_args)
        .current_dir(&repo_root)
        .env(ARTIFACT_ENV, &worker)
        .status()
        .map_err(|error| format!("failed to launch SilicoLab: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("SilicoLab exited with {status}."))
    }
}

pub fn build() -> Result<PathBuf, String> {
    let repo_root = repo_root()?;
    ensure_rust_target(&repo_root)?;
    let rust_lld = bundled_rust_lld(&repo_root)?;
    let linker_dir = rust_lld
        .parent()
        .ok_or_else(|| format!("could not resolve parent of {}", rust_lld.display()))?;

    println!("Building silicolab-compute for {TARGET}");
    let mut command = Command::new("cargo");
    command
        .args([
            "build",
            "--release",
            "--package",
            "silicolab-compute",
            "--target",
            TARGET,
        ])
        .current_dir(&repo_root)
        .env("CARGO_TARGET_DIR", repo_root.join("target"));
    prepend_path(&mut command, linker_dir)?;
    let status = command
        .status()
        .map_err(|error| format!("failed to build development worker: {error}"))?;
    if !status.success() {
        return Err(format!("development worker build failed with {status}."));
    }

    let worker = repo_root
        .join("target")
        .join(TARGET)
        .join("release")
        .join("silicolab-compute");
    validate_linux_x86_64_elf(&worker)?;
    println!("Development worker: {}", worker.display());
    Ok(worker)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn smoke_test_version(worker: &Path) -> Result<(), String> {
    let output = Command::new(worker)
        .arg("--version")
        .output()
        .map_err(|error| format!("failed to run worker {}: {error}", worker.display()))?;
    if !output.status.success() {
        return Err(format!(
            "worker version smoke test failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let reported = String::from_utf8(output.stdout)
        .map_err(|error| format!("worker produced non-UTF-8 version output: {error}"))?;
    let expected = env!("CARGO_PKG_VERSION");
    if reported.trim() != expected {
        return Err(format!(
            "worker reports version `{}` but `{expected}` was expected",
            reported.trim()
        ));
    }
    Ok(())
}

fn ensure_rust_target(repo_root: &Path) -> Result<(), String> {
    let args = ["target", "list", "--installed"];
    let output = Command::new("rustup")
        .args(args)
        .current_dir(repo_root)
        .output()
        .map_err(|error| format!("failed to run rustup {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        print_command_failure("rustup", &args, &output);
        return Err(format!(
            "rustup target query failed with {}.",
            output.status
        ));
    }

    let targets = String::from_utf8(output.stdout)
        .map_err(|error| format!("rustup produced non-UTF-8 target output: {error}"))?;
    if targets.lines().any(|target| target.trim() == TARGET) {
        return Ok(());
    }

    println!("Installing Rust target {TARGET}");
    let status = Command::new("rustup")
        .args(["target", "add", TARGET])
        .current_dir(repo_root)
        .status()
        .map_err(|error| format!("failed to install Rust target {TARGET}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("rustup target installation failed with {status}."))
    }
}

fn bundled_rust_lld(repo_root: &Path) -> Result<PathBuf, String> {
    let sysroot = command_stdout(repo_root, "rustc", &["--print", "sysroot"])?;
    let version = command_stdout(repo_root, "rustc", &["-vV"])?;
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or_else(|| "rustc -vV did not report its host toolchain".to_owned())?;
    let linker = Path::new(sysroot.trim())
        .join("lib")
        .join("rustlib")
        .join(host)
        .join("bin")
        .join(format!("rust-lld{}", env::consts::EXE_SUFFIX));
    if linker.is_file() {
        Ok(linker)
    } else {
        Err(format!(
            "the active Rust toolchain does not contain its bundled linker at {}",
            linker.display()
        ))
    }
}

fn command_stdout(repo_root: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(repo_root)
        .output()
        .map_err(|error| format!("failed to run {program} {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        print_command_failure(program, args, &output);
        return Err(format!(
            "{program} {} failed with {}.",
            args.join(" "),
            output.status
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("{program} produced non-UTF-8 output: {error}"))
}

fn prepend_path(command: &mut Command, directory: &Path) -> Result<(), String> {
    let current_path = env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(directory.to_path_buf()).chain(env::split_paths(&current_path));
    let path = env::join_paths(paths)
        .map_err(|error| format!("could not add {} to PATH: {error}", directory.display()))?;
    command.env("PATH", path);
    Ok(())
}

fn validate_linux_x86_64_elf(path: &Path) -> Result<(), String> {
    let mut file = fs::File::open(path)
        .map_err(|error| format!("failed to open worker {}: {error}", path.display()))?;
    let mut header = [0_u8; 64];
    file.read_exact(&mut header)
        .map_err(|error| format!("failed to read ELF header from {}: {error}", path.display()))?;
    validate_elf_header(&header)
        .map_err(|error| format!("worker {} is invalid: {error}", path.display()))
}

fn validate_elf_header(header: &[u8]) -> Result<(), String> {
    if header.len() < 64 || header.get(..4) != Some(b"\x7fELF") {
        return Err("expected an ELF executable".to_owned());
    }
    if header[4] != 2 {
        return Err(format!("expected ELF64 class, found {}", header[4]));
    }
    if header[5] != 1 {
        return Err(format!(
            "expected little-endian ELF data, found {}",
            header[5]
        ));
    }
    let object_type = u16::from_le_bytes([header[16], header[17]]);
    if !matches!(object_type, 2 | 3) {
        return Err(format!(
            "expected an executable or position-independent executable, found ELF type {object_type}"
        ));
    }
    let machine = u16::from_le_bytes([header[18], header[19]]);
    if machine != 62 {
        return Err(format!(
            "expected x86-64 architecture, found ELF machine {machine}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_elf_header;

    fn elf_header(object_type: u16) -> [u8; 64] {
        let mut header = [0_u8; 64];
        header[..4].copy_from_slice(b"\x7fELF");
        header[4] = 2;
        header[5] = 1;
        header[6] = 1;
        header[16..18].copy_from_slice(&object_type.to_le_bytes());
        header[18..20].copy_from_slice(&62_u16.to_le_bytes());
        header
    }

    #[test]
    fn accepts_x86_64_executable_and_static_pie_headers() {
        assert!(validate_elf_header(&elf_header(2)).is_ok());
        assert!(validate_elf_header(&elf_header(3)).is_ok());
    }

    #[test]
    fn rejects_wrong_elf_format_or_architecture() {
        let mut header = elf_header(2);
        header[4] = 1;
        assert!(validate_elf_header(&header).unwrap_err().contains("ELF64"));

        let mut header = elf_header(2);
        header[5] = 2;
        assert!(
            validate_elf_header(&header)
                .unwrap_err()
                .contains("little-endian")
        );

        let mut header = elf_header(2);
        header[18..20].copy_from_slice(&183_u16.to_le_bytes());
        assert!(validate_elf_header(&header).unwrap_err().contains("x86-64"));
    }

    #[test]
    fn rejects_non_executable_or_truncated_input() {
        assert!(
            validate_elf_header(&elf_header(1))
                .unwrap_err()
                .contains("executable")
        );
        assert!(validate_elf_header(b"\x7fELF").is_err());
    }
}
