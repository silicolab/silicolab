---
title: 安装
description: 使用预构建可执行文件安装 SilicoLab，或从源码构建。
sidebar:
  order: 1
---

## 预构建可执行文件

从 [GitHub Releases](https://github.com/silicolab/silicolab/releases)
下载对应平台的可执行文件即可运行，无需安装程序。

## 从源码构建

安装 [Rust 工具链](https://rustup.rs)，然后构建 release 版本：

```sh
cargo build --release
```

生成的二进制文件位于 `target/release/`（Linux/macOS 下为 `silicolab`，
Windows 下为 `silicolab.exe`）。

## 可选的外部工具

部分功能在运行时会调用外部程序。这些程序可以之后再安装——在用到
相应功能之前，SilicoLab 无需它们也能正常运行。

- **GROMACS** —— 运行分子动力学模拟时必需。
- **ORCA** —— 用于量子化学计算。

这些工具的完整配置指南（含 GPU 加速与 SSH 远程计算）将在本手册的
后续章节中提供。
