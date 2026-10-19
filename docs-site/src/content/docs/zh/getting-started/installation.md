---
title: 安装
description: 使用预构建可执行文件安装 SilicoLab，或从源码构建。
sidebar:
  order: 1
---

## 下载与安装

从 [GitHub Releases](https://github.com/silicolab/silicolab/releases) 下载。
macOS 13 及以上用户按处理器选择：Apple Silicon 使用 `macos-aarch64.dmg`，
Intel 使用 `macos-x86_64.dmg`。打开后将左侧的 SilicoLab 大图标拖到右侧
Applications，推出磁盘映像，再从 Finder 的 Applications 启动，无需终端命令。

当前 DMG 内的应用采用 **ad-hoc 签名，未公证**。本地签名完整性检查和本地启动
不能替代互联网下载后的安装验收。Gatekeeper 可能阻止下载的副本；请以实际提示
和 [Apple 应用安全说明](https://support.apple.com/zh-cn/102445)为准，不要关闭
Gatekeeper，也不要忽视恶意软件或应用损坏警告。

更新 DMG 安装时先退出 SilicoLab，再从新 DMG 替换整个应用；项目和设置会保留。
Windows/Linux 便携压缩包及原有 macOS 压缩包目录结构仍保留原有更新行为。

项目仍是 SQLite 数据库目录，请通过应用内 **Open Project** 打开。`.sls` 脚本仍需
显式从 CLI 或应用内执行，不注册为 Finder 自动执行的文件。本安装包不包含外部
计算引擎，也不改变远程 worker 的部署方式。

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
