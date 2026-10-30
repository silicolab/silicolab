---
title: Quickstart
description: Launch the GUI, or run a script headless from the command line.
sidebar:
  order: 2
---

## Launch the GUI

Run SilicoLab with no arguments to open the graphical interface:

```sh
silicolab
```

From the GUI you can view and edit 3D structures; sketch molecules in 2D;
run geometry optimizations, quantum chemistry calculations, and molecular
dynamics simulations; and plot the results.

If your shell cannot find `silicolab`, run the executable with its full path or
add the directory containing it to your `PATH`. Source builds place the binary
in `target/release/`.

## Run a script headless

Pass a script path to run it from the command line, without the GUI:

```sh
silicolab workflow.sls
```

By default the script runs in a throwaway workspace and keeps nothing after it
exits. To build on earlier results, run it inside a saved project:

```sh
silicolab --project path/to/project workflow.sls
```

The project is saved after every line, so a long script that is stopped part
way keeps the steps that finished. A project can be open in only one place at
a time: the command refuses a project that a SilicoLab window or another script
already has open.

To see where a project stands, print its status as JSON:

```sh
silicolab status --project path/to/project
```

The report lists the project's entries and tasks. Each completed task is marked
`current`, or `stale` with the reasons: an input structure was edited or deleted
after the task ran, the structure it produced was edited, or a task it builds on
is itself out of date. `status` only reads the project, so it also works while
the project is open in a window, and it shows what was last saved.

An AI coding agent can drive a long workflow this way: run `status`, run the next
script with `--project`, and run `status` again. When the agent is interrupted or
starts a new session, `status` tells it what is already done and which results
need to be recomputed, so it does not have to remember.

The same `.sls` scripts also run interactively in the GUI console, so a
workflow you automate on the command line behaves identically inside the app.

## Next steps

Follow this route from setup to remote compute:

1. [Installation](../installation/) — install a prebuilt executable or build
   SilicoLab from source.
2. [Interface tour](../interface-tour/) — learn where structures, actions,
   tasks, and output appear.
3. [First project](../first-project/) — create, save, close, and reopen a
   persistent project.
4. [External tools](../external-tools/) — configure optional engines only when
   a workflow needs them.
5. [Remote execution](../remote-execution/) — run supported compute jobs on a
   remote Linux host.
