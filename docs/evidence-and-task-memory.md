# Evidence and task memory

This document owns the project record protocol and its extension contract.
The implementation is `backend/records`, with dispatcher adapters in
`frontend/dispatcher/records` and retrieval in `frontend/agent/tools/records`.

## Authority and ownership

A project has one `RecordStore`, owned by its `RunGraph`. This ownership reuses
project cloning, switching, saving and loading; a memory record does **not** need
a job. There is no second mutable query index: queries scan the validated,
in-memory records. Scratch projects use exactly the same queries.

The envelope contains a stable ID, category, source, scope, creation time,
revision, lifecycle relations, bounded brief, typed content, historical entry
references and registered artifacts. Projects scope the entire store; sessions
and tasks narrow applicability. Task scope includes its run UUID. Three version
concepts are distinct:

- `storage_version` versions the serialized record envelope (currently 1).
- `content_version` versions the selected typed content (currently 1).
- `revision` identifies an immutable fact/interpretation revision. A replacement
  gets a new ID and explicitly names `supersedes`; it never overwrites facts.

`Evidence` requires a program source and execution identity. `Memory` currently
supports user-approved constraints. `Derived` supports non-authoritative
explanations with generator, model and explicit `(record ID, revision)` bases.
Missing, replaced or invalidated bases make a derived record stale, including
through other derived records. Dependency cycles loaded from a project are stale;
new interpretations cannot reference stale or cyclic bases. Traversal is iterative
so deeply nested imported dependencies cannot overflow the call stack. No automatic
model summarization or history compression is introduced.

The job graph remains the authority for execution state. `QmResult` remains the
single authority for reported convergence and artifact-write status; record
queries join it rather than copying it into the numerical facts. Query-index
availability and project commit status are reported separately. A recorded
artifact-write success describes the write, not a guarantee that a file still
exists; explicit raw reads report later filesystem errors.

## QM adapter

GUI, Agent, synchronous commands and remote completion use the same deterministic
adapter from `QmOutcome`. Launch sites capture the actual typed `QmJob`, including
geometry, method/basis, charge/spin and other applicable parameters. It is stored
as `input:<job ID>`; numerical facts at `qm:<job ID>` reference that snapshot.
There is no lookup of the current editor configuration during result application.
Older executions without snapshots explicitly have no input-record reference.

Numerical facts include energy in hartree, sample availability/counts, non-finite
counts, and a finite frequency range in inverse centimetres. Empty samples mean
“not supplied”, not “not calculated”. Moving-geometry SCF traces may describe
only the final geometry. Non-finite energy is unavailable with a reason. No
stationary-point or empirical-threshold classification is performed. Non-finite
series are not written as invalid JSON chart data: series status reports the
write rejection while compact facts retain availability and non-finite counts.

Reports and chart series for new jobs live at
`runs/<task directory>/jobs/<job ID>/`. Remote download directories also isolate
QM jobs. Entry and task viewers resolve reports and series independently from
their registrations; an unavailable artifact does not fall back to shared files
when the job has evidence records. Legacy
`output.txt` remains locatable by the workspace view, explicitly without exact
job attribution; it is never served as a registered report for an old job.

Method-quality warnings do not yet have complete typed coverage. Compact facts
and completion messages always state this and direct the reader to the raw
report. Report and numerical-series SHA-256 fingerprints detect conflicting
replays. These checks precede artifact writes and do not depend on readable QM
status. A conflicting replay is rejected before status or materialization changes.
Identical outcomes may repair failed artifact writes or missing/damaged status; existing facts
and the materialization ledger prevent overwrites and duplicate geometries.

## Persistence and recovery

`knowledge_records(id, envelope_json)` is added by the existing incremental
schema initialization. It deliberately has no cascading foreign key to the
rewritten execution table. Records, executions and attempts are written in the
same caller transaction; full saves also include entries, the materialization
ledger and conversations. Narrow graph saves defer to pending full saves.
User-approved constraints request a full save so their session is saved with
its record. Failed commits leave dirty state and explicitly report memory-only
success, never persistence success.

Unknown content/envelope versions or damaged records are quarantined per row,
retain their original serialized bytes through saves, and are retrievable via
`unavailable`. Damaged QM status JSON is similarly retained while the status
becomes unknown. Project opening reports these degradations. If conversation state is damaged,
fresh conversation IDs reserve the IDs already referenced by records, so an
unrelated new conversation cannot inherit old constraints. An old database
gets an empty catalog without changing its existing results.

First save / Save As copies registered artifacts into the destination project's
`runs/` tree and changes only the copied task paths. It does not move or delete
the original evidence. Copy errors abort that save and leave the source state
intact. Retrying accepts byte-identical copies but refuses conflicting files;
new artifact writes use a flushed staging file and rename. Unregistered legacy files keep their existing project behavior.

A crash before a project transaction commits may leave orphan files. The database
never treats those files alone as imported results. A downloaded, typed remote
outcome can replay through the normal adapter to recover absent facts or repair
failed artifacts. Legacy task-level outcome files shared by multiple executions
are rejected as ambiguous; recovery must retrieve the particular job again.
Quarantined unknown-version records and their artifacts are never overwritten by
an older adapter. Without a reliable typed outcome, recovery reports that it
cannot reconstruct numerical facts; `output.txt` is never parsed for numbers.
Local computations lost before commit cannot be fully reconstructed from chart
series alone. Artifact writes and SQLite transactions are separate durability
boundaries; this is not a cross-filesystem transaction system.

## Retrieval and explicit confirmation

`inspect` remains read-only. No arguments means the workspace view, not historical
record injection. Example tool arguments:

```json
{"view":"catalog","category":"evidence","task":7,"offset":0,"limit":20}
{"view":"summary","job":"<job UUID>","content_type":"qm.facts"}
{"view":"details","id":"input:<job UUID>","detail_offset":0}
{"view":"raw","id":"qm:<job UUID>","artifact":"report","byte_offset":0}
{"view":"catalog","category":"memory","session":1,"lifecycle":"active"}
{"view":"unavailable","offset":20}
{"view":"intent","detail_offset":0}
```

Filters combine with AND. A named record conflicting with another filter is an
error. Unknown IDs never fall back to recent results. Directory/summary pages
sort by `(created_at_ms, id)` and return a continuation offset; follow the returned
offset, since the output budget can shorten a page below its requested limit.
Offsets assume the record set is unchanged between pages. Details paginate the
serialized typed record by character offset. Raw access requires an exact record
and registered artifact and returns byte offsets aligned to UTF-8 boundaries.
The raw tool reads at most 481 bytes per call, including lookahead; there is no
whole-file read followed by truncation. Encoded responses reserve space within
the Agent tool-result budget. No arbitrary filesystem path is accepted.

`save_constraint` accepts `text`, optional `task`, and optional `replaces`. The
existing `ApproveToolCall` action is the sole production approval path. Every
call asks, even in Auto mode or with permanent allow rules. The card displays
the exact text, scope and replacement ID; it does not offer permanent approval.
There is no model-settable confirmation flag. Missing explicit replacement keeps
both potentially conflicting constraints active. Constraint scope is always the
current project and conversation, optionally narrowed to one task.

Request assembly adds a bounded current user instruction, applicable active
constraints and current-task evidence references. `intent` retrieves the complete
latest original user instruction when its context copy was shortened; it does
not promote that message to a confirmed constraint. Omitted applicable records are
explicitly counted and remain inspectable. Completion, diagnostic and queued
messages contain bounded descriptions and references, never an automatic QM
report. Existing unconverged/missing-artifact diagnosis remains read-only.

## Extending

For another actual compute type, add a versioned typed `Content` variant and its
validation and deterministic adapter. Capture immutable conditions at launch,
register artifacts under the existing execution's run directory, and call the
adapter from that engine's existing result-application paths. Extend authoritative
job-result status only if the domain needs it. Storage, scopes, lifecycle,
pagination, artifact reads and context selection do not require QM fields.
A test-only measurement adapter exercises this independence.

Intentionally absent: vector search, relevance learning, automatic explanations,
full history compression and UI memory management. Context restores the latest
user instruction, not a separately curated long-term goal model. Evidence
records retain compact numerical facts and typed inputs, not full local outcomes.
