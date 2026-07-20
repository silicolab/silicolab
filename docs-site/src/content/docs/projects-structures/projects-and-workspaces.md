---
title: Projects and workspaces
description: Understand Scratch, persistent project directories, save checkpoints, drafts, and safe project moves.
sidebar:
  order: 1
---

## Understand the workspace boundary

SilicoLab starts in the **Scratch temporary workspace**. Scratch is useful for
viewing, importing, or preparing structures without first choosing a project
directory. Its contents are temporary: closing SilicoLab without creating a
project does not preserve the Scratch workspace.

| Workspace | How to recognize it | Persistence boundary |
| --- | --- | --- |
| Scratch | The welcome area is titled `Scratch temporary workspace` | It has no project database. Creating a project carries the current committed Scratch entries and task state into the new project |
| Project | The title bar shows the project name | Entries, project task records, and saved application state are stored under the selected project root |

A SilicoLab project is a directory, not a single project file. Its root can also
refer to run data outside that directory, so it is not automatically a
self-contained archive. Do not edit the databases or other SilicoLab-managed
files inside the project directory by hand.

## Create a project without replacing a directory

1. Choose **File > Create a new project...**.
2. Select a writable parent directory and enter a unique project name.
3. Before confirming, make sure the resulting target directory does **not**
   already exist.
4. Confirm creation, then check that the title bar shows the new project name.

The project chooser may look like a file-save dialog, but SilicoLab uses the
chosen name as a new directory. If a same-named directory exists, or the system
shows a replace or overwrite prompt, choose **Cancel**. Use a unique name or an
empty parent directory instead. Do not use project creation to adopt, merge
with, or replace an existing directory.

Creating a project from Scratch is not necessarily an empty-project operation.
Committed Scratch entries and task state are carried into the new project. Open
editors and unsubmitted forms still have their own Apply, Build, Run, or Cancel
boundary and should be resolved first.

## Open an existing project

| Goal | Action | Check after the action |
| --- | --- | --- |
| Open a project | Choose **File > Open Project...** and select the project root itself | The title bar shows the project name; a saved current entry appears in the central viewport, or an empty project opens with an empty viewport |
| Reopen a recent project | Choose its name under **File > Recent Projects** | The title bar changes to that project and the expected entries are available |

Select the existing project root, not its parent, an internal child directory,
or a structure file. A Recent Projects item is only a remembered path; it does
not guarantee that the directory still exists or remains valid.

If opening reports `not a valid SilicoLab project`, cancel the operation and
locate the actual project root. If the project moved, open its current root
directly. Do not create a same-named directory at the old path to hide a stale
recent item.

When the transient status says
`recovered: previous session did not close cleanly`, inspect Entries, the
current structure, Activity, and important results before editing. This message
only identifies an unclean prior shutdown; it does not prove that data was
repaired or that every external job and path is intact. Preserve a copy of the
whole project root before attempting recovery if anything looks wrong.

## Treat Save Project as an explicit checkpoint

SilicoLab writes some committed changes in the background, but not every open
interface draft is committed state. There is also no single visible indicator
that proves every relevant item is safely stored.

After important edits, and before switching projects or quitting, choose
**File > Save Project**. A successful explicit save posts the temporary status
`Saved project <name>`. That message can disappear as newer feedback arrives.
Waiting, seeing no warning, or looking at Console is not evidence that the save
succeeded.

If the status reports `Project save failed:` or a leave confirmation reports
`Could not save project`, stay in the current project. Check write permission
and available storage, then save again. System-level save failures can be
reviewed later in **Output** with the **System** source selected; successful
project saves are not recorded there as a durable receipt.

## Know where feedback appears

- The status area shows temporary confirmations such as `Opened project`,
  `Saved project`, `Closed project`, and successful build messages.
- **Activity** is the visible monitor for current and previous jobs.
- **Console** contains the transcript of commands entered through the `.sls`
  command interface. It is not a general action log.
- **Output** contains non-command logs. Use its **System** source to revisit
  recorded application, project, storage, or file failures.

Use the resulting project name, entries, job state, and files as the durable
checks. Do not expect a temporary success message to remain available.

## Resolve drafts before saving or leaving

**Warning:** A live preview is not the same as a committed edit. Resolve every
open editor or form before saving, opening another project, closing the project,
or quitting.

- In **Structure Editor**, choose **Apply** to commit the complete editor
  session, or **Cancel** to restore the structure from before the editor opened.
  Confirm that the editor closes and the viewport matches your choice.
- In a task panel, use its own **Build**, **Start**, **Run**, or **Cancel**
  control. A successful build normally posts temporary status feedback; inspect
  the resulting entry, task panel, result location, or **Activity** rather than
  expecting Console or Output to preserve that success message.
- Close or cancel other open dialogs, including Sketch and Export, if they are
  not meant to continue.

The leave confirmation does not replace these decisions. When Scratch contains
work, it offers **Create Project**, **Discard**, and **Cancel**. For a persistent
project it can offer **Save and Open**, **Save and Close**, or **Save and Quit**,
alongside **Don't Save** and **Cancel**. If saving fails, SilicoLab keeps the
leave action unresolved; correct the failure before trying again.

## Close and reopen safely

Choose **File > Close Project** to return to Scratch. The welcome area shows
`Scratch temporary workspace` again. The temporary status
`Closed project; opened Scratch` confirms the transition at that moment; it is
not a deletion and is not a durable log entry.

Reopening a saved project can restore open tabs, their per-entry views, and the
current entry. Sidebar multi-selection and group expansion are not a reliable
cross-session scope. Before any batch action, recheck the highlighted rows,
visible groups, current tab, and central viewport.

## Back up or move a project cautiously

Task records belong to the workspace that started them, but all required run
data need not live inside the project root. Local or remote run directories,
detached-job records, trajectories, docking poses, and other artifacts can carry
their own path or host requirements.

**Warning:** Do not move or rename a project while local or remote work is
active. A project's stored identity can survive a directory move, but that does
not guarantee that every job association or saved run path will be relocated.
SilicoLab does not provide a blanket promise to rewrite external paths.

For a backup or planned move:

1. Resolve open drafts and run **Save Project**. Confirm the temporary saved
   status while it is visible.
2. In **Activity**, confirm that no relevant local or remote job is active or
   cancelling.
3. Keep the original location available. Copy the complete project root and any
   required external run directories or artifacts.
4. Open the copied root and inspect Entries, the current structure, Activity,
   Output, and the expected result locations.
5. Keep the original and external paths until the workflows that matter have
   been reopened and checked. Opening the copy alone does not prove that every
   stored reference is independent of the old path.

## Related tasks

- [Create and reopen a first persistent project](../../getting-started/first-project/)
- [Understand entries, groups, and action scope](../entries-and-groups/)
