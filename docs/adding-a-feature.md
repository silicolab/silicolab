# Adding a feature

This codebase is pattern-based: nearly every feature is a near-mechanical mirror
of an existing one. To add one, find the closest archetype below, open its
**template module**, and copy its shape across the layers. This page maps
archetypes to templates and lists the cross-cutting wiring for the most common
one (a background compute task). It points at templates rather than restating
them — read the named module for the current code.

See [ARCHITECTURE.md](../ARCHITECTURE.md) for *why* the layers and the
`AppAction → dispatch` flow exist, and [CONTRIBUTING.md](../CONTRIBUTING.md) for
build/test/land. Changes to a remote-capable engine or the `wire`/`payload`
contract must also follow
[Developing remote execution](developing-remote-execution.md) so the current
worker is rebuilt and exercised instead of a released worker. Layer order
(lowest owns it) spans two crates: **compute-core**
`domain → io → md → engines → workflows` (no GUI deps; also holds `hosts`/
`payload`/`wire`; `md` is the engine-neutral molecular-dynamics model), then
**silicolab** `backend → frontend` (depends on compute-core); the headless
worker crate `silicolab-compute` links compute-core alone. The module paths
below are relative to a crate: `md/`, `engines/`, `io/`, `workflows/` live under
`compute-core/src/`; `backend/`, `frontend/` under `src/`.

## Pick the template

| You are adding… | Mirror | Notes |
|---|---|---|
| In-process pure-Rust compute | `engines/qm/` (+ `engines/forcefield/uff`) | Free functions, **no `Engine` trait**; register a built-in `EngineId` + capability in `engines/registry.rs`. |
| External subprocess engine | `engines/gromacs/` (+ `engines/process`, `engines/remote`) | Declare an `EngineSpec` in `registry.rs`; runs on a `JobManager` thread, never the UI thread. |
| Run an existing engine on a **remote** host | `engines/remote/` (`deploy`, `launcher`, `run_record`) + `frontend/dispatcher/` (`remote_jobs`, `builders`) | Mirror the `resolve_remote_compute_host → start_remote_engine → apply_remote_*_outcome` dispatch; the job crosses the `wire`/`payload` bridge to the deployed `silicolab-compute` worker. Reuses each engine's `compute-core` impl, so local and remote can't diverge. Rebuild and test it with `cargo xtask remote-dev`. |
| Background compute **task** (panel + result entries) | the `qm-energy` task, end-to-end — see the checklist | For a CPU loop that streams intermediate structures, also study `build-disordered-system`. |
| Multi-entry picker panel | `disorder` | `SetDisorderComponentEntry`, `with_disorder_prompt`, and the `ensure_panel_form` seeding. |
| Inline edit task (no panel, acts on the active structure) | `add-hydrogens` / `recompute-bonds` | Executor runs the op and marks the task complete; no prompt state. |
| New `.sls` command group | `frontend/qm_commands.rs` | Sub-dispatch + a `--flag` scanner; register the verb in `console.rs`. Defining it once makes it a CLI command too. |
| New molecular file format | `io/formats/cif.rs` (single) / `mol2.rs`, `pdb.rs` (multi-record) | Plus 3 registry edits — see Traps. Declare whether records concatenate in `StructureFormat::multi_structure_file`. |
| New GUI interaction (no task) | an `AppAction` variant + arm in `dispatcher/mod.rs` + a handler in a `dispatcher/*.rs` | Widgets only emit actions; only `dispatch` mutates state. |
| Expose a capability to the in-app assistant | — | Automatic via the `run_command` tool once a `.sls` verb exists. Only gate the verb in `agent/tools.rs::command_needs_confirmation`, and heavy-classify it in `agent/loop_driver/heavy.rs` if it is CPU-bound. |

## Background compute task — the full checklist

Mirror `qm` bottom-up. The compiler forces most of these (non-exhaustive `match`
arms won't build); ⚠ marks the steps it does **not** catch.

1. **`engines/<name>/`** — request/outcome types (mirror the external crate's
   enums so its types never reach silicolab's API edge) + `run_<name>(request,
   cancel, report)`. `pub mod` in `engines/mod.rs`; built-in `EngineId` +
   capability in `engines/registry.rs`.
2. **`workflows/<name>.rs`** — a thin `run_<name>_calculation` + a progress type.
   `pub mod` in `workflows/mod.rs`.
3. **`backend/tasks.rs`** — a `TaskKind`, a `TaskPanelKind`, and a
   `TaskController` in `TASK_CONTROLLERS`. Persistence is automatic (only
   `controller_id` is stored; no schema change).
4. ⚠ **`frontend/task_executor.rs`** — a `TaskExecutor` + `run_<x>_panel`. The
   `every_task_controller_has_frontend_executor` test fails if you forget this.
5. **`frontend/jobs.rs`** — `RunningXJob` + `XWorkerMessage` + `spawn_x_job`, a
   `JobManager` slot + accessors, and a `JobControlId`/`LocalJobSlot` arm in the
   unified job control plane used by `AppState::cancel_transient_jobs`.
6. **`frontend/state/<x>_prompts.rs`** — the draft struct; `mod`/`pub use` in
   `state.rs`; a `pending_x` field on `UiState` (struct + init +
   `restore_edit_snapshot`).
7. **`frontend/dispatcher/<x>.rs`** — `start_pending_x`,
   `cancel_pending_x_request`, `poll_x_job`; `mod` + `pub(crate) use` in
   `dispatcher/mod.rs`; an `ensure_panel_form` arm in `dispatcher/tasks.rs`; and
   a `poll_x_job` call in `poll_jobs`.
8. **`frontend/actions.rs`** — `StartX` / `CancelXPrompt`, with dispatch arms in
   `dispatcher/mod.rs`.
9. **`frontend/ui/secondary_sidebar/task_panels.rs`** — `render_x_task_panel`
   (`pub(crate)`), dispatched from `frontend/ui/dock.rs::render_task_body`.
10. **`frontend/<x>_commands.rs`** — the `.sls` command; `mod` in
    `frontend/mod.rs`; an arm in `console.rs` plus the `command_catalog()` /
    `help_text()` strings.
11. **Assistant** — gate the verb in `agent/tools.rs`; if CPU-bound, add
    `HeavyKind` + `AgentHeavyJob` arms in `agent/loop_driver/heavy.rs`.

Result entries are created with `add_and_show_entry` (best result, preserves the
active task run) and `EntryStore::add_entry_to_group`; record provenance with a
new `EntryOrigin` variant.

## Traps the compiler/tests won't catch

- **A normal app launch does not test unpublished remote worker changes.** If a
  remote-capable engine, `EngineRequest`, `EngineOutcome`, or the payload bridge
  changes, rebuild and run the current worker with `cargo xtask remote-dev`.
  Use `cargo xtask build-dev-worker` in an IDE pre-launch task and run the
  relevant opt-in SSH test described in the
  [remote development guide](developing-remote-execution.md).
- **A new IO format = 4 sites, and one is not a `match`.** Touch
  `io/structure_format.rs` (the enum + the `READABLE_FORMATS`/`WRITABLE_FORMATS`
  arrays + `label`/`extension`/`from_extension`), `io/structure_codec.rs`,
  `io/formats/mod.rs`, and ⚠ `io/structure_paths.rs::READABLE_EXTENSIONS` — a
  plain `[&str; N]` array that nothing flags if it goes stale.
- **`frontend/{state,dispatcher,ui}/dock.rs` is the GUI panel-docking system**
  (DockArea/DockTab), unrelated to anything chemical. Don't reuse the `Dock*`
  prefix for a domain feature (molecular docking uses `Docking*`).
- **A "one opaque blocking call" engine (hartree, Vina) can't be preempted.**
  `cancel` is best-effort: honored before the call starts; in-flight work runs to
  completion and the result is discarded. Don't promise mid-run cancellation.
- **Console commands are shared GUI+CLI.** Define the command once in
  `console.rs`; never add a GUI-only path.

---

*Templates are named by their current example (`qm`, `mol2`, `disorder`, …). If
those modules are renamed, update this file.*
