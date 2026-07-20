---
title: Entries and groups
description: Distinguish entries, groups, the current entry, sidebar selection, and atom selection, then use scoped actions safely.
sidebar:
  order: 2
---

## Keep five states separate

The **Entries** sidebar presents organization, selection, and activation at the
same time. These states can point to the same structure, but they have different
roles.

| Concept | What it represents | Visible check | Do not treat it as |
| --- | --- | --- | --- |
| Entry | One structure or result-bearing project item | A row in Entries, inside a group or ungrouped | A source file or a task run |
| Group | A container used to organize entries | A group header with an expanded or collapsed marker | An operation that changes the member structures |
| Current entry | The one entry displayed in the central viewport and used as the default context for ordinary single-structure tasks | Its tab is current and its structure is visible | Every highlighted sidebar entry |
| Sidebar-selected entries or groups | The highlighted rows used by Export and sidebar batch actions | One or more entry rows or group headers are highlighted | The current entry or atom selection |
| Selected atoms | Atom indices selected inside the current entry | Atoms highlight in the viewport and, when compatible metadata exists, in Sequence | Sidebar entry multi-selection |

Changing sidebar selection does not necessarily change the central viewport.
Changing atom selection does not change the sidebar entry scope.

## Select or activate entries

| Action | State change | Visible check |
| --- | --- | --- |
| Single-click an entry | Replace the sidebar selection with that row without activating it | The row highlights; the viewport can continue to show the previous current entry |
| Double-click an entry | Select and activate that entry | Its tab becomes current, the viewport switches to its structure, and a temporary `Loaded entry <name>` status appears |
| `Cmd`-click on macOS or `Ctrl`-click on Windows/Linux | Toggle an entry in the existing sidebar selection | The row enters or leaves the highlighted scope without changing the viewport |
| `Shift`-click | Replace selection with the range from the anchor to that item in the current visible order | The visible entry rows and group headers in the range highlight |

Shift ranges include only rows in the current visible order. Members of a
collapsed group and rows hidden by a search are excluded. Expand the group or
clear the search, then select again if those entries should be in scope.

## Organize entries with groups

Choose **New Group**, enter a non-empty name, and choose **Create**. The new
group header appears in Entries and a temporary `Created group <name>` status
confirms the action.

A plain click on a group header selects the group and toggles its expanded or
collapsed state. A `Cmd`/`Ctrl`-click or `Shift`-click changes only sidebar
selection and does not toggle expansion. Use **Collapse All** to hide all member
rows without deleting entries.

Groups organize entries; they do not merge their structures. Removing a group
does not remove its members unless the destructive delete-with-entries action is
chosen explicitly.

## Identify the input scope before acting

| Action type | Actual input | Check before continuing |
| --- | --- | --- |
| Ordinary single-structure task | The current entry, not sidebar multi-selection | Put the target structure in the central viewport, then verify the input shown in the task panel |
| **Export...** | The selected entry rows plus every member of a selected group | In Export Structures, check **Selected (N)** and the previewed names |
| Batch entry action | Explicitly selected entry rows | Check the dynamic command label, such as **Delete N Entries** |
| Group action | Explicitly selected group headers | Check whether the command says **Ungroup N Groups** or **Delete N Groups and Their Entries** |
| Multi-input task | The selectors inside that task's own panel | Verify each displayed input name; sidebar highlighting does not replace task-specific selectors |

After starting a job, use its panel or **Activity** to follow it. **Console** is
only the `.sls` command transcript, and **Output** carries non-command logs; a
sidebar highlight is not task-input evidence in either view.

## Ungroup while keeping entries

**Ungroup**, **Ungroup 1 Group**, and **Ungroup N Groups** remove only group
containers. Before choosing one, inspect the highlighted group headers and the
dynamic count. Afterward, the headers disappear and their member entries remain
in the ungrouped area. The current entry is not deleted merely because its group
was removed.

The small trash control on a group row is also an Ungroup action. Use the
separately named delete-with-entries command when deletion is actually intended.

## Delete entries

**Warning:** **Delete Entry**, **Delete 1 Entry**, and **Delete N Entries**
remove project entries immediately, without an additional confirmation. Before
choosing the command, verify the highlighted entry rows and the count in its
label. The structure shown in the viewport does not prove the delete scope.

After deletion, the row disappears and temporary status feedback identifies the
deleted entry. This removes the entry from the project; it does not state that
an external source file or every task artifact associated with it was deleted.

If the deleted entry was current, the viewport may switch to another open tab or
become empty. Double-click an intended remaining entry to restore a usable
current context. That does not recover the deleted entry.

## Delete groups and their entries

**Delete Group and All Entries**, **Delete 1 Group and Its Entries**, and
**Delete N Groups and Their Entries** are destructive. They use a two-step
confirmation that identifies the group or group count and the number of entries
to be removed.

Choose **Cancel** to disarm the confirmation and leave the groups and entries in
place. Choose **Delete** only after both counts match your intent. The operation
removes the project groups and member entries; it does not claim to delete
external source files or external run directories.

## Related tasks

- [Understand project directories, save checkpoints, and reopen behavior](../projects-and-workspaces/)
- [Select entries, atoms, and sequence residues](../selection-and-sequence/)
- [Edit and export structures](../edit-and-export/)
