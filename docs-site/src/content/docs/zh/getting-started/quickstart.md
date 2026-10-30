---
title: 快速上手
description: 启动图形界面，或在命令行中无界面运行脚本。
sidebar:
  order: 2
---

## 启动图形界面

不带参数运行 SilicoLab 即可打开图形界面：

```sh
silicolab
```

在图形界面中，你可以查看和编辑 3D 结构、绘制 2D 分子草图、运行几何优化、
量子化学计算与分子动力学模拟，并对结果绘图。

如果 shell 找不到 `silicolab`，请使用可执行文件的完整路径运行，或将其所在目录
加入 `PATH`。从源码构建的二进制文件位于 `target/release/`。

## 无界面运行脚本

传入脚本路径即可在命令行中运行，无需打开图形界面：

```sh
silicolab workflow.sls
```

默认情况下，脚本在一个临时工作区中运行，退出后不保留任何内容。要在之前的
结果上继续，可以让脚本在已保存的项目中运行：

```sh
silicolab --project path/to/project workflow.sls
```

每执行完一行就会保存项目，所以长脚本中途被停止时，已经完成的步骤会保留下来。
同一个项目同一时间只能在一处打开：如果某个 SilicoLab 窗口或另一个脚本已经打开
了该项目，命令会拒绝运行。

要查看项目当前的进展，可以用 JSON 输出项目状态：

```sh
silicolab status --project path/to/project
```

报告会列出项目里的条目和任务。每个已完成的任务会标为 `current`（仍然有效），
或者标为 `stale`（已过期）并给出原因：任务运行后，它的输入结构被修改或删除了；
它生成的结构被修改了；或者它所依赖的上游任务本身已经过期。`status` 只读取项目，
所以项目在窗口中打开时也能使用，显示的是最近一次保存的内容。

AI 编程助手可以用这种方式驱动较长的流程：先运行 `status`，再用 `--project` 运行
下一个脚本，然后再运行一次 `status`。助手被中断或换了新会话时，`status` 会告诉
它哪些步骤已经完成、哪些结果需要重新计算，不需要靠它自己记住。

由 AI 助手运行脚本时，加上 `--strict`，让 SilicoLab 直接拒绝错误的操作，而不是
依赖助手自己避免：

```sh
silicolab --project path/to/project --strict step.sls
```

严格模式下，每个计算都必须写明它使用的条目。校验选项放在命令前面：

```text
--entry #3@2 --request-id opt-1 qm optimize --method r2scan-3c
```

- `--entry #3@2` 在条目 3 上运行；如果它已经不是第 2 版（`status` 报告的
  revision），就拒绝运行。需要多个输入的计算（例如对接）可以重复写 `--entry`，
  第一个会成为当前条目。
- 如果输入来自已过期的任务，或者来自没有完成的任务，会被拒绝。加上
  `--allow-stale` 可以照样使用，任务会记录这一点。
- `--request-id` 让重试变得安全：同一个 id 的任务已经完成时，命令只报告它，
  不会再算一次。

严格模式还会把没有收敛的 QM 计算标为失败；只有当条目的 `md build` 对应它当前的
revision 时才运行 MD；写文件（`export`、`save`、`fetch --dir`）只能写在项目目录
里。这些校验选项不加 `--strict` 也能在脚本和控制台里使用，只是在严格模式下是
必需的。

支持 Model Context Protocol（MCP）的助手可以通过工具而不是 shell 命令来操作项目。
把 SilicoLab 注册为 MCP 服务器，例如在 Claude Code 中：

```sh
claude mcp add silicolab -- silicolab mcp --project path/to/project
```

服务器提供三个工具：`project_status`（即 `status` 报告）、`run_commands`（运行
控制台命令，始终按严格模式校验）和 `command_catalog`（命令参考）。每次调用都会
打开项目、保存并关闭，所以两次调用之间仍然可以在应用里打开这个项目。计算会在
调用返回之前完成。

同样的 `.sls` 脚本也可以在图形界面的控制台中交互执行——在命令行里自动化
的工作流，在应用内的行为完全一致。

## 下一步

按以下路线从安装走到远程计算：

1. [安装](../installation/)——下载预构建可执行文件，或从源码构建 SilicoLab；
2. [界面导览](../interface-tour/)——了解结构、操作、任务和输出分别出现在哪里；
3. [创建第一个项目](../first-project/)——创建、保存、关闭并重新打开持久化项目；
4. [外部工具](../external-tools/)——仅在工作流需要时配置可选引擎；
5. [远程执行](../remote-execution/)——在远程 Linux 主机上运行受支持的计算任务。
