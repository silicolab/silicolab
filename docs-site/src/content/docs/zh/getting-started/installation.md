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

量子化学计算默认使用内置 Hartree 引擎。ORCA 可作为分子单点能、几何优化和
振动频率计算的可选外部引擎，必须由用户指定可执行文件路径。

请阅读[外部工具](../external-tools/)了解配置说明，包括 GPU 加速。
如需通过 SSH 在远程 Linux 主机上运行大型任务，请阅读[远程执行](../remote-execution/)。
