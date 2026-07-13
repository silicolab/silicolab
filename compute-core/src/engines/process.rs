//! Generic subprocess execution layer used by external engine integrations.
//!
//! `engines::process` provides the foundation for running command-line tools
//! (such as GROMACS) with streamed stdout/stderr, a
//! cooperative cancellation flag, and a wall-clock timeout. It deliberately
//! avoids any async runtime so that engine jobs can be spawned from worker
//! threads owned by the calling application rather than an async runtime.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};

/// Description of an external command to execute.
#[derive(Debug, Clone)]
pub struct ProcessConfig {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    pub env: HashMap<String, String>,
    pub timeout: Option<Duration>,
    /// Optional bytes to feed to the child's stdin. When `None` (the default)
    /// the child inherits a null stdin, matching the historical behavior. Many
    /// GROMACS analysis tools (`genion`, `gmx energy`, `trjconv`, `rms`, ...)
    /// prompt for group/term selections that are supplied this way, mirroring
    /// the tutorial's `printf "...\n" | gmx ...` idiom.
    pub stdin: Option<Vec<u8>>,
}

impl ProcessConfig {
    pub fn new(executable: impl Into<PathBuf>, working_dir: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            working_dir: working_dir.into(),
            env: HashMap::new(),
            timeout: None,
            stdin: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Feed `bytes` to the child's stdin verbatim, then close it (EOF).
    pub fn stdin_bytes(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }

    /// Feed `lines` to the child's stdin, each terminated by `\n` (including a
    /// trailing newline after the last line), then close it. This matches the
    /// `printf "SOL\n"` style of selection feeding used by GROMACS tools.
    pub fn stdin_lines(self, lines: &[&str]) -> Self {
        let mut payload = String::new();
        for line in lines {
            payload.push_str(line);
            payload.push('\n');
        }
        self.stdin_bytes(payload.into_bytes())
    }
}

/// One streamed event emitted from a running subprocess.
#[derive(Debug, Clone)]
pub struct ProcessEvent {
    pub timestamp: Instant,
    pub kind: ProcessEventKind,
}

#[derive(Debug, Clone)]
pub enum ProcessEventKind {
    Stdout(String),
    Stderr(String),
    Exited(i32),
}

/// Aggregated result emitted once the subprocess has terminated.
#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub wall_time: Duration,
    pub timed_out: bool,
    pub cancelled: bool,
}

impl ProcessResult {
    pub fn success(&self) -> bool {
        !self.timed_out && !self.cancelled && self.exit_code == 0
    }
}

/// Handle to a running subprocess. Drop to detach; call [`ProcessHandle::join`]
/// to block until completion and retrieve the [`ProcessResult`].
pub struct ProcessHandle {
    receiver: Option<Receiver<ProcessEvent>>,
    pub cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<Result<ProcessResult>>>,
}

impl ProcessHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Borrow the streaming receiver. Useful for consumers that want to peek
    /// at events while leaving the handle alive.
    pub fn receiver(&self) -> Option<&Receiver<ProcessEvent>> {
        self.receiver.as_ref()
    }

    /// Detach the streaming receiver from the handle so it can be moved into
    /// a separate log-drain thread independently of [`Self::join`].
    pub fn take_receiver(&mut self) -> Option<Receiver<ProcessEvent>> {
        self.receiver.take()
    }

    pub fn join(mut self) -> Result<ProcessResult> {
        self.join
            .take()
            .ok_or_else(|| anyhow!("subprocess handle has already been joined"))?
            .join()
            .map_err(|_| anyhow!("subprocess worker panicked"))?
    }
}

/// Spawn the command described by `config` and return a streaming handle.
pub fn spawn(config: ProcessConfig) -> Result<ProcessHandle> {
    spawn_with_cancel(config, Arc::new(AtomicBool::new(false)))
}

/// Spawn with an externally owned cancellation flag so callers can share it
/// with other systems (for example, the existing optimization control).
pub fn spawn_with_cancel(config: ProcessConfig, cancel: Arc<AtomicBool>) -> Result<ProcessHandle> {
    std::fs::create_dir_all(&config.working_dir).with_context(|| {
        format!(
            "failed to create working directory {}",
            config.working_dir.display()
        )
    })?;

    let mut command = Command::new(&config.executable);
    command
        .args(&config.args)
        .current_dir(&config.working_dir)
        .stdin(if config.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &config.env {
        command.env(key, value);
    }

    suppress_console(&mut command);

    let mut child = command.spawn().with_context(|| {
        format!(
            "failed to spawn {} in {}",
            config.executable.display(),
            config.working_dir.display()
        )
    })?;

    // Feed stdin from a dedicated thread so a child that exits before draining
    // its input cannot deadlock us against a full pipe buffer. Dropping the
    // handle after writing signals EOF.
    if let Some(payload) = config.stdin.clone()
        && let Some(mut child_stdin) = child.stdin.take()
    {
        thread::spawn(move || {
            let _ = child_stdin.write_all(&payload);
            let _ = child_stdin.flush();
            // `child_stdin` drops here, closing the pipe (EOF).
        });
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to capture child stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("failed to capture child stderr"))?;

    let (sender, receiver) = mpsc::channel::<ProcessEvent>();
    let started_at = Instant::now();

    let stdout_sender = sender.clone();
    let stdout_thread =
        thread::spawn(move || pump_lines(stdout, stdout_sender, StreamKind::Stdout));
    let stderr_sender = sender.clone();
    let stderr_thread =
        thread::spawn(move || pump_lines(stderr, stderr_sender, StreamKind::Stderr));

    let cancel_for_worker = Arc::clone(&cancel);
    let exit_sender = sender;
    let timeout = config.timeout;

    let join = thread::spawn(move || -> Result<ProcessResult> {
        let mut timed_out = false;
        let mut cancelled = false;

        let exit_status = loop {
            if let Some(status) = child
                .try_wait()
                .context("failed to poll child process status")?
            {
                break status;
            }
            if cancel_for_worker.load(Ordering::Relaxed) {
                cancelled = true;
                let _ = child.kill();
                break child.wait().context("failed to reap cancelled child")?;
            }
            if let Some(limit) = timeout
                && started_at.elapsed() >= limit
            {
                timed_out = true;
                let _ = child.kill();
                break child.wait().context("failed to reap timed-out child")?;
            }
            thread::sleep(Duration::from_millis(50));
        };

        let stdout = stdout_thread
            .join()
            .map_err(|_| anyhow!("stdout reader panicked"))?;
        let stderr = stderr_thread
            .join()
            .map_err(|_| anyhow!("stderr reader panicked"))?;
        let exit_code = exit_status.code().unwrap_or(-1);

        let _ = exit_sender.send(ProcessEvent {
            timestamp: Instant::now(),
            kind: ProcessEventKind::Exited(exit_code),
        });

        Ok(ProcessResult {
            exit_code,
            stdout,
            stderr,
            wall_time: started_at.elapsed(),
            timed_out,
            cancelled,
        })
    });

    Ok(ProcessHandle {
        receiver: Some(receiver),
        cancel,
        join: Some(join),
    })
}

/// Convenience: run a command to completion and return its aggregated result.
pub fn run(config: ProcessConfig) -> Result<ProcessResult> {
    spawn(config)?.join()
}

/// A GUI-subsystem process has no console, so Windows allocates a fresh console
/// *window* for every console-subsystem child it spawns (`wsl.exe`, `ssh.exe`,
/// `gmx.exe`). This layer always pipes the child's stdio, so that window can
/// never carry anything: suppressing it is a property of the layer, not a knob.
///
/// `creation_flags` overwrites the flag word rather than OR-ing into it; any
/// future flag must be folded in here rather than set at a second call site.
#[cfg(windows)]
fn suppress_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn suppress_console(_command: &mut Command) {}

#[derive(Debug, Clone, Copy)]
enum StreamKind {
    Stdout,
    Stderr,
}

fn pump_lines<R: std::io::Read>(
    reader: R,
    sender: Sender<ProcessEvent>,
    kind: StreamKind,
) -> String {
    let mut buffered = String::new();
    let reader = BufReader::new(reader);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        buffered.push_str(&line);
        buffered.push('\n');
        let event_kind = match kind {
            StreamKind::Stdout => ProcessEventKind::Stdout(line),
            StreamKind::Stderr => ProcessEventKind::Stderr(line),
        };
        let _ = sender.send(ProcessEvent {
            timestamp: Instant::now(),
            kind: event_kind,
        });
    }
    buffered
}

/// Locate an executable on `PATH`. On Windows the standard `PATHEXT` extensions
/// (`.exe`, `.bat`, `.cmd`, `.com`) are appended automatically when the bare
/// candidate is not a file.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    if let Some(direct) = direct_path(name) {
        return Some(direct);
    }

    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if let Some(found) = lookup_in_dir(&dir, name) {
            return Some(found);
        }
    }
    None
}

fn direct_path(name: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(name);
    if candidate.components().count() <= 1 {
        return None;
    }
    if candidate.is_file() {
        return Some(candidate);
    }
    if cfg!(windows) {
        for ext in windows_path_extensions() {
            let mut with_ext = candidate.clone().into_os_string();
            with_ext.push(ext);
            let probed = PathBuf::from(with_ext);
            if probed.is_file() {
                return Some(probed);
            }
        }
    }
    None
}

fn lookup_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let candidate = dir.join(name);
    if candidate.is_file() {
        return Some(candidate);
    }
    if cfg!(windows) {
        for ext in windows_path_extensions() {
            let mut with_ext = candidate.clone().into_os_string();
            with_ext.push(ext);
            let probed = PathBuf::from(with_ext);
            if probed.is_file() {
                return Some(probed);
            }
        }
    }
    None
}

fn windows_path_extensions() -> Vec<String> {
    let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.BAT;.CMD;.COM".to_string());
    raw.split(';')
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn echo_streams_stdout_and_exits() {
        let (executable, args) = if cfg!(windows) {
            (
                PathBuf::from("cmd.exe"),
                vec!["/C".to_string(), "echo hello".to_string()],
            )
        } else {
            (
                PathBuf::from("sh"),
                vec!["-c".to_string(), "echo hello".to_string()],
            )
        };

        let working_dir = std::env::temp_dir().join("silicolab_process_echo_test");
        let config = ProcessConfig::new(executable, working_dir).args(args);
        let result = run(config).expect("subprocess runs to completion");

        assert!(result.success(), "expected success, got {result:?}");
        assert!(result.stdout.to_lowercase().contains("hello"));
    }

    #[test]
    fn timeout_kills_long_running_process() {
        let (executable, args, timeout) = if cfg!(windows) {
            (
                PathBuf::from("cmd.exe"),
                vec!["/C".to_string(), "ping 127.0.0.1 -n 5 >NUL".to_string()],
                Duration::from_millis(150),
            )
        } else {
            (
                PathBuf::from("sh"),
                vec!["-c".to_string(), "sleep 2".to_string()],
                Duration::from_millis(150),
            )
        };

        let working_dir = std::env::temp_dir().join("silicolab_process_timeout_test");
        let config = ProcessConfig::new(executable, working_dir)
            .args(args)
            .timeout(timeout);
        let result = run(config).expect("subprocess runs to completion");

        assert!(result.timed_out, "expected timeout, got {result:?}");
        assert!(!result.success());
    }

    #[test]
    fn stdin_is_delivered_to_child() {
        // A child that echoes whatever it reads from stdin back to stdout. On
        // Windows `sort` reads all of stdin and writes the sorted lines; on
        // Unix `cat` copies stdin verbatim. Either way the payload must appear
        // in stdout, proving stdin was delivered.
        let (executable, args) = if cfg!(windows) {
            (
                PathBuf::from("cmd.exe"),
                vec!["/C".to_string(), "sort".to_string()],
            )
        } else {
            (PathBuf::from("cat"), Vec::new())
        };

        let working_dir = std::env::temp_dir().join("silicolab_process_stdin_test");
        let config = ProcessConfig::new(executable, working_dir)
            .args(args)
            .stdin_lines(&["SOL", "0"]);
        let result = run(config).expect("subprocess runs to completion");

        assert!(result.success(), "expected success, got {result:?}");
        assert!(
            result.stdout.contains("SOL"),
            "expected stdin to round-trip to stdout, got {:?}",
            result.stdout
        );
    }

    #[test]
    fn stdin_writer_does_not_deadlock_when_child_ignores_it() {
        // Feed a payload larger than a typical pipe buffer (64 KiB) to a child
        // that exits immediately without reading stdin. The dedicated writer
        // thread must not wedge `join()`.
        let (executable, args) = if cfg!(windows) {
            (
                PathBuf::from("cmd.exe"),
                vec!["/C".to_string(), "exit 0".to_string()],
            )
        } else {
            (
                PathBuf::from("sh"),
                vec!["-c".to_string(), "exit 0".to_string()],
            )
        };

        let working_dir = std::env::temp_dir().join("silicolab_process_stdin_deadlock_test");
        let large = vec![b'x'; 256 * 1024];
        let config = ProcessConfig::new(executable, working_dir)
            .args(args)
            .stdin_bytes(large)
            .timeout(Duration::from_secs(10));
        let result = run(config).expect("subprocess runs to completion");

        assert!(!result.timed_out, "writer thread deadlocked the join");
        assert!(result.success(), "expected clean exit, got {result:?}");
    }

    #[test]
    fn stdin_lines_appends_trailing_newline_per_line() {
        let config = ProcessConfig::new("noop", ".").stdin_lines(&["SOL", "0"]);
        assert_eq!(config.stdin.as_deref(), Some(b"SOL\n0\n".as_slice()));
    }
}
