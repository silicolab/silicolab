//! Every external process in `compute-core` must be spawned through
//! `engines::process`, which suppresses the console window Windows pops up for a
//! console-subsystem child of a GUI process. A stray `Command::new` bypasses that
//! and reintroduces the flicker — invisibly, since the app crate is
//! `windows_subsystem = "windows"` only on Windows and CI runs headless Linux.

use std::path::Path;

/// The only sanctioned `Command::new` outside `engines/process.rs`: a call that
/// launches our own executable, which is GUI-subsystem and never gets a console.
const SELF_LAUNCHING: &[&str] = &["wire.rs", "io/self_update.rs"];

#[test]
fn subprocesses_are_spawned_only_through_engines_process() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();

    visit_rust_files(&src, &mut |path| {
        let relative = path
            .strip_prefix(&src)
            .expect("walked path is under src")
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "engines/process.rs" || SELF_LAUNCHING.contains(&relative.as_str()) {
            return;
        }
        let text = std::fs::read_to_string(path).expect("read source file");
        for (index, line) in text.lines().enumerate() {
            if line.contains("Command::new") {
                offenders.push(format!("{relative}:{}", index + 1));
            }
        }
    });

    assert!(
        offenders.is_empty(),
        "`Command::new` outside `engines::process`:\n  {}\n\n\
         Build a `ProcessConfig` and call `engines::process::run`/`spawn` instead. \
         Spawning a console program directly from the GUI pops up a console window \
         on Windows. If the call launches our own executable, add it to \
         `SELF_LAUNCHING` in this test.",
        offenders.join("\n  ")
    );
}

fn visit_rust_files(dir: &Path, visitor: &mut dyn FnMut(&Path)) {
    for entry in std::fs::read_dir(dir).expect("read source directory") {
        let path = entry.expect("read directory entry").path();
        if path.is_dir() {
            visit_rust_files(&path, visitor);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            visitor(&path);
        }
    }
}
