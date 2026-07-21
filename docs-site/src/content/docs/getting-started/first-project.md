---
title: Create your first project
description: Create, build, save, and reopen a persistent project from a fixed benzene SMILES.
sidebar:
  order: 4
---

## Goal

Starting from a fresh, empty **Scratch temporary workspace**, create the persistent
project `SilicoLab Manual Demo`, build the fixed SMILES as the active `Benzene` entry,
save the project, and close and reopen it. The final visible checkpoint is that
`C6H6`, 12 atoms, and 12 bonds remain after reopening.

## Fixed sample

- Input: inline SMILES `c1ccccc1`. It is not a Sample ID and requires no sample download.
- Starting state: SilicoLab is open at an empty **Scratch temporary workspace**; **Entries** contains no entries, and no persistent project is open.

## Prerequisites

- Complete [Installation](../installation/) and confirm that the GUI launches.
- Read the [Interface tour](../interface-tour/) and distinguish an entry selection from the active entry.
- Start from a fresh, empty Scratch workspace. If SilicoLab opens a project, choose **File > Close Project**; if Scratch contains entries, restart SilicoLab and confirm that **Entries** is empty before continuing.
- Have a writable parent directory in which `SilicoLab Manual Demo` does not yet exist. This tutorial needs no ORCA, GROMACS, or other external tool.

## Actions

### 1. Create a persistent project

Before opening the dialog, confirm that **Entries** is empty and that
`<parent>/SilicoLab Manual Demo` does not exist. Choose
**File > Create a new project…**. In the system save dialog, enter
`SilicoLab Manual Demo` in **Save As:**, select the writable parent directory, and
confirm. SilicoLab creates a project root with that name inside the selected parent.

If the save dialog reports a name conflict or asks whether to replace an existing
item, choose **Cancel**. Never confirm replacement. Select a new, empty parent
directory, or restart with a unique project name and use that name in later checks.

**Observation:** The title bar shows `SilicoLab Manual Demo`, the workspace is no
longer Scratch, and a transient status notice reads
`Opened project SilicoLab Manual Demo`.

### 2. Open the molecule sketcher

Choose **File > Sketch Molecule…**.

**Observation:** The **Sketch Molecule** window opens. While the sketch is empty,
**Build (Save as New)** is disabled.

### 3. Import the fixed SMILES

Enter `c1ccccc1` in the **SMILES** field, then choose **Import**.

**Observation:** The status reads `Imported SMILES (6 atoms)`. These are the six
carbon heavy atoms in the SMILES sketch; implicit hydrogens are materialized only
during Build.

### 4. Name and build the entry

Enter `Benzene` in **Title:**, then choose **Build (Save as New)**.

**Observation:** The new entry automatically becomes active and appears in the
central viewport. The status bar reads `Benzene | 12 atoms | 12 bonds`; **Details**
shows `Formula: C6H6`; and a transient status notice reads
`Built sketched molecule as entry #1 (12 atoms)`.

### 5. Save the project

Choose **File > Save Project**.

**Observation:** A transient status notice reads `Saved project SilicoLab Manual Demo`.

### 6. Close the project

Choose **File > Close Project**.

**Observation:** The application returns to **Scratch temporary workspace**, and
a transient status notice reads `Closed project; opened Scratch`.

### 7. Reopen it from recent projects

Choose **File > Recent Projects > SilicoLab Manual Demo**.

**Observation:** The title bar returns to `SilicoLab Manual Demo`, and `Benzene`
returns as the active entry. The status bar reads `Benzene | 12 atoms | 12 bonds`.
Details shows `Atoms: 12`, `Bonds: 12`, and `Formula: C6H6`. A separate transient
status notice reads `Opened project SilicoLab Manual Demo`.

## Expected output

The output is a persistent project directory named `SilicoLab Manual Demo`, not a
single project file. Its project databases record the project name and the `Benzene`
entry title, 12 atoms, and 12 bonds; this tutorial does not require opening or editing
those databases. After closing and reopening the project, all of these interface
states should be restored:

- `Benzene` is the active entry and appears in the central viewport.
- Details shows 12 atoms, 12 bonds, and formula `C6H6`.
- The status bar reads `Benzene | 12 atoms | 12 bonds`.

## Recovery

| Symptom | Check | Recovery |
| --- | --- | --- |
| Scratch already contains entries | This walkthrough needs an empty Scratch workspace so the built molecule becomes entry #1. | Do not continue. Restart SilicoLab, close any automatically opened project if needed, and confirm that **Entries** is empty. |
| The save dialog reports a name conflict or asks to replace an item | A target with that project name already exists in the selected parent directory. | Choose **Cancel** and never confirm replacement. Select a new, empty parent directory, or restart with a unique project name and use that name in later checks. |
| Import reports `SMILES error:` | Check that the input exactly matches `c1ccccc1`. | Correct the input and choose **Import** again. Do not Build an old or empty sketch. |
| **Build (Save as New)** remains disabled | Check whether Import succeeded and the status reads `Imported SMILES (6 atoms)`. | Re-enter the fixed SMILES and choose **Import**; Build only after the success status appears. |
| The system save dialog points to the wrong parent directory | Check the current directory before confirming the dialog. | Choose **Cancel**, start the create-project action again, and select the correct writable parent. Do not use directory deletion as recovery. |
| The `Benzene` row is selected, but another entry remains in the central viewport | Check whether the row is merely selected rather than active. | Double-click `Benzene` in Entries to activate it. |
| `SilicoLab Manual Demo` is absent from **Recent Projects** | Confirm that the project root still exists. | Choose **File > Open Project…** and select the `SilicoLab Manual Demo` project root. |

## Scientific interpretation limits

**Build (Save as New)** adds implicit hydrogens, attempts built-in UFF relaxation
from multiple initial 3D geometries, and keeps the lowest-energy successful result.
This does not make the result an experimental structure, a complete conformer search,
a research-grade method validation, or evidence that it is suitable for a specific
scientific problem.

## Next steps

- [Choose the next page from the quickstart roadmap](../quickstart/)
- [Configure external tools only when a workflow needs them](../external-tools/)
