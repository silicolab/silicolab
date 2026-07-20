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
