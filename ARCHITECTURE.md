# Architecture

SilicoLab is a Rust (edition 2024) **Cargo workspace** for molecular and
materials modeling. It is split into three crates:

- **`silicolab`** (repository root, `src/`) — the desktop application: an
  egui/wgpu GUI and a headless CLI, both driven by the same `.sls` scripting
  language defined in `src/frontend/console.rs`.
- **`compute-core`** (`compute-core/`) — the compute stack (domain model, IO,
  engines, workflows) plus the remote-host descriptor, the serializable payload
  bridge, and the engine-job wire contract. It carries **no GUI dependencies**.
- **`silicolab-compute`** (`silicolab-compute/`) — the headless compute worker, a
  self-contained static musl binary deployed to a remote host on first use. It
  links `compute-core` alone.

A script command is defined **once** in `console.rs` and is then available in
both the GUI console and the CLI. When you add or change a command, you do not
wire it up twice — keep that single definition the source of truth so the two
front-ends can never drift apart.

This document captures the design invariants that are not obvious from reading
any single file. They hold regardless of who (or what) is editing the code;
respect them so changes don't quietly break the architecture. For how to build,
test, and land changes, see [CONTRIBUTING.md](CONTRIBUTING.md); for where to put
a new feature and which module to mirror, see
[docs/adding-a-feature.md](docs/adding-a-feature.md).

## Entry point

`main.rs` dispatches on its arguments: no args → launch the GUI; a `.sls` path →
run that script headless. Both paths converge on the same script/command layer,
which is what makes the dual-front-end guarantee above hold.

## Module layers

Code is organized as a one-directional stack; a lower layer never depends on a
higher one. The stack spans two crates — the GUI app depends on `compute-core`,
never the reverse:

```
compute-core crate (no GUI deps):
  domain/    pure data, no UI or IO
    → io/        file formats + outbound transport (PDB fetch, update check, LLM)
    → md/        engine-neutral molecular-dynamics model (system, topology,
                 solvation, force fields, the stage/parameter model)
    → engines/   compute (local subprocess engines + the remote launcher)
    → workflows/ composed operations
  (+ hosts        remote-host descriptor
     payload      serializable Structure <-> wire bridge
     wire         engine-job request/outcome contract)

silicolab crate (depends on compute-core):
    → backend/   persistence (SQLite projects, config, secrets)
    → frontend/  egui / wgpu, the .sls console, the dispatcher

silicolab-compute crate (depends on compute-core):
    the headless worker that runs wire requests on a remote host
```

**Why:** keeping `domain/` free of UI and IO — and `compute-core` free of GUI
deps entirely — makes the core data model testable, reusable by both front-ends,
and linkable by the headless worker alone. The strict direction prevents
dependency cycles and keeps compute and persistence independent of rendering.
When adding code, put it in the lowest layer (and lowest crate) that can own it,
and don't reach upward.

## GUI data flow: single direction, single mutator

The GUI is an Elm-style loop:

- UI code under `frontend/ui/` is **pure rendering**. It reads `AppState` and
  emits `AppAction`s (`frontend/actions.rs`). It never mutates application state.
- **Only `dispatcher.rs::dispatch` mutates `AppState`.**
- A new GUI operation = add an `AppAction` variant → handle it in
  `dispatcher.rs` → emit it from the widget.

**Why:** funneling every state change through one function gives a single,
auditable place where state transitions happen. That is what makes undo/replay,
persistence, and reasoning about "what changed and why" tractable. If widgets
mutated state directly, those transitions would be scattered across the render
code and impossible to track.

### Exception: transient render state

There is one deliberate, narrow exception for **transient/derived render
state** — fields that are:

1. recomputed every frame from other state or from system queries,
2. never persisted, and
3. never subject to undo or replay.

Such fields may be written directly in the app loop (`app.rs`) or in the
rendering pass itself, bypassing `AppAction → dispatch`.

Current example: `UiState::glass_active`, derived each frame from `config.glass`
plus an OS accessibility check.

Everything else is **persisted application state** and must go through
`AppAction → dispatch`. Before adding a field to this exception, confirm all
three criteria hold: pure derivation, frame-scoped, no semantic history. When in
doubt, route it through dispatch.

**Why this distinction is first-class:** the line between "derived render state"
and "persisted application state" is the most load-bearing decision in the GUI,
and it is wrong to err in either direction. Routing genuinely per-frame values
through dispatch bloats the action and undo machinery; letting persisted state
skip dispatch silently breaks undo, replay, and save.

### In-progress editor sessions

A second, related case is an **in-progress editing session** that is not yet
committed to the workspace: the structure editor (`StructureEditor`) and the 2D
molecule sketcher (`SketcherState`), both held as `Option<…>` fields on
`UiState`. These are not per-frame derived state — they live across frames and
carry their own internal undo/redo — but the *draft* they hold is still
transient: it is mutated directly by the rendering code as the user edits (drag
an atom, draw a bond), and **only the committed result crosses
`AppAction → dispatch`** (e.g. `ApplyStructureEdits`, `CommitSketch`, which
build the new/edited entry through the normal entry machinery). The session's
own undo stack is local to the session and is discarded when it closes; the
workspace-level history only records the single committed change. Keep this
shape for any future "draft then commit" editor: own the draft in `UiState`,
mutate it directly while open, and route only open/commit/cancel through the
dispatcher.

## Engine discovery is performance-sensitive

`registry.rs::probe()` is cheap: it only checks `PATH` and configured overrides
and spawns no subprocess. `detect_versions()` and `probe_with_versions()` are
**slow** — they run each engine's `--version`, which cold-starts WSL.

Run the slow paths only when the user explicitly asks to detect or refresh
engine versions. Never trigger them on routine events such as opening the
settings panel.

**Why:** cold-starting WSL on every settings-panel open would make the UI feel
broken. Cheap probing is safe to do liberally; version detection is an explicit,
user-initiated action.

## Subprocess execution

The subprocess layer (`engines/process.rs`) is **async-runtime-free** and runs
only on `JobManager` worker threads — never on the UI thread. Cancellation is via
an `AtomicBool`, and runaway processes are bounded by a wall-clock timeout.

**Why:** external compute engines can run for minutes and must never block
rendering. Keeping this layer free of an async runtime keeps it simple and
portable, and confining it to worker threads guarantees the UI stays responsive
and cancellable.

## Launching engines: WSL vs native

Some engines (notably GROMACS for molecular dynamics) run inside WSL. They are
launched with `command_prefix = ["wsl.exe", "-e"]` and a Linux program path.
Natively installed engines use an empty prefix. This prefix model lets a single
launch path serve both WSL-hosted and natively installed tools.

## Remote execution

A compute job can be offloaded to a remote Linux host over SSH instead of run
locally. This is the same `compute-core` job machinery, transported:

- A job is serialized to the **wire** contract (`compute-core/src/wire.rs`) with
  its `Structure` carried by the **payload** bridge (`payload.rs`); the host is
  described by `hosts`.
- The headless **worker** (`silicolab-compute`) has two explicitly separated
  artifact sources. Ordinary builds resolve the asset from the exact GitHub
  Release tag for their package version and verify its published SHA-256.
  Contributor builds with `dev-worker` resolve a validated local musl binary,
  identify its actual bytes as `dev:<sha256>`, and resolve that artifact without
  contacting GitHub Releases or falling back to a released worker. Official
  builds do not compile the local source path.
- `engines/remote/deploy.rs` installs either artifact through one fail-closed
  path: upload an identity-qualified binary, mark it executable, verify its
  reported package version, and only then atomically replace the stable symlink.
  The persisted `_worker` value is the deployment identity (a package version
  for production or `dev:<sha256>` for development), not an assumption that a
  path still exists. Even an identity cache hit is checked on the host; a
  missing or invalid executable is redeployed and a stale or unverifiable
  worker is never run. Jobs invoke the verified identity-qualified path rather
  than the mutable stable symlink.
- The worker executes the request and writes an outcome the client reads back.
  QM, docking, and GROMACS/MD all run through this path; jobs are bounded by the
  host's own CPU and RAM.

**Why:** the wire/payload split keeps the GUI-free `compute-core` as the single
implementation of each engine — local and remote runs share it, so behavior can
not drift between them. Separating production and development artifact sources
keeps released deployment pinned and checksum-verified while allowing current
source to be tested without weakening that invariant. Fail-closed deployment
means a tampered, missing, or mismatched worker fails the job rather than
silently producing wrong results. Contributor commands and host-test setup live
in [docs/developing-remote-execution.md](docs/developing-remote-execution.md).

## Storage model

A project is a **directory of SQLite databases** (`backend/storage.rs`, via
rusqlite) — not a single file and not a custom binary format. Treat the project
directory as the unit of a project.

## Updates

Updating is split in two so detection is cheap and acting is opt-in:

- `io/update_check.rs` only **detects** — one anonymous GitHub Releases query
  per launch, compared against the compiled-in version. It never downloads.
- `io/self_update.rs` **acts** — it downloads the release asset matching this
  platform's target triple and replaces the running executable via the
  `self_update` crate (which handles Windows's "can't overwrite a running exe"
  through an atomic self-replace). `is_self_update_supported()` probes whether
  the install directory is writable; portable / package-manager installs fall
  back to the releases page instead of offering a one-click update that would
  only error.

Both run on `JobManager` worker threads (`spawn_update_check`,
`spawn_self_update`) and report back through the usual channel-poll pattern, so
the UI thread never blocks on network or disk. The default flow is **one-click
manual**: the title bar surfaces an "Update" button that becomes a "Restart"
button once installed. The `auto_install_updates` preference (off by default)
makes a discovered update download itself; `maybe_auto_install_update` gates
both the toggle and the background poll on the same conditions.
