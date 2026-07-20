---
title: Interface tour
description: Learn the SilicoLab workbench regions, active-entry and selection states, and where actions put their results.
sidebar:
  order: 3
---

## Read the workbench

The title bar shows the current project context. Application menus are in the title
bar on Windows and Linux, and in the system menu bar on macOS. The primary sidebar
switches among **Entries**, **Launch**, and **Style**; the central viewport shows only
the active entry. The right and bottom areas are movable docks, so saved layouts may
differ.

| Location | Use it for | Where to look after an action |
| --- | --- | --- |
| Title bar | Confirm the project name; open application menus here on Windows and Linux, or in the system menu bar on macOS. | It confirms project context, not that a task or calculation has completed. |
| Primary sidebar | Manage structure entries in **Entries**, start tasks in **Launch**, and adjust the current structure's display in **Style**. | New structures commonly appear in Entries; a task and its status are not the active entry. |
| Central viewport | Display the active entry and atom selections within that structure. | A structure appears here only when it is the active entry. |
| Bottom dock | **Console** contains only the command transcript and provides the `sls>` input; **Sequence** shows sequence information for the current structure; **Activity** shows task status; **Output** contains non-command **System**, **Agent**, **Remote**, and **Jobs** logs; **Plot** shows available result charts. | Check Activity and the relevant Output source first, then the entry, task result panel, report, or Plot as the task requires. |
| Right dock | **Assistant** opens here by default for conversational preparation or launch of supported actions. | Confirm assistant-started work in Activity, the relevant Console or Output record, and the applicable result location. |

## Distinguish three states

- **Entry selection** is the row or rows selected in Entries for entry-level actions.
- The **active entry** is the one entry loaded in the central viewport and used as the current structure context. A click can change only the entry selection; double-click an entry to activate it.
- **Atom selection** is the set of atoms selected within the active entry for operations such as styling and structure editing.

All three can sometimes point into the same structure, but they are not the same state.

## Restore the default docks

On first launch, the primary sidebar defaults to Entries. The bottom dock contains
Console, Sequence, Activity, Output, and Plot in that order, with Console active;
Assistant is active at right.

If a persisted layout is hard to recognize, choose **View > Reset Workbench Layout**.
This restores the default docks but preserves the primary sidebar view that was active
before the reset, so select **Entries** afterward.

## Find an action's result

Actions do not share one result route: a task may create a new entry, modify a structure
in place, write only files, or generate a report. After an action, check **Activity**
status, changes in **Entries**, and the task result panel, report, **Plot**, or relevant
log surface. **Console** contains the command transcript and provides the `sls>` input.
**Output** contains non-command **System**, **Agent**, **Remote**, and **Jobs** logs; it
is not a copy of Console.

## Limits

UI state and a **Completed** task marker do not validate method choice, calculation
quality, or a scientific conclusion.

## Next steps

- [Return to the quickstart to review the GUI and command-line entry points](../quickstart/)
