---
title: 外部工具
description: 配置 SilicoLab 各功能模块调用的外部程序。
sidebar:
  order: 3
---

SilicoLab 在没有可选外部工具时也可以启动。只有在使用对应功能时，才需要安装
这些程序。

## ORCA

量子化学计算默认使用内置 Hartree 引擎。ORCA 是可选引擎，首版支持分子体系的
单点能、几何优化和振动频率计算；过渡态与周期 QM 计算仅支持 Hartree。

请单独安装 ORCA，然后打开 **设置 > 计算目标**，在本机或远程主机的 ORCA 行中
填写该目标可用的可执行文件路径，再点击 **验证**。SilicoLab 不会自动搜索或选择
ORCA。在 Windows 中使用 WSL 版 ORCA 时，将命令前缀设为 `wsl.exe -e`，程序填写
WSL 内的通用 Linux 路径，例如 `/opt/orca/orca`；原生安装则将命令前缀留空。

在 QM 任务面板中显式选择 ORCA，或在脚本中使用 `qm energy --engine orca`。
ORCA 默认使用一个 CPU 核心；请求更多核心会启用 `%pal` 并行模式，并要求目标
环境中能够调用 `mpirun`。

## GROMACS

分子动力学模拟需要单独安装 [GROMACS](https://www.gromacs.org/)。

强烈建议使用 GPU 加速。只用 CPU 运行分子动力学在技术上可行，但对于稍复杂的
体系通常会非常慢。

- **Windows：** 在 [WSL](https://learn.microsoft.com/en-us/windows/wsl/install)
  中安装 GROMACS。快速开始可用 `sudo apt install gromacs`。如需 GPU 加速，
  请在 WSL 中从源码编译带 CUDA 支持的 GROMACS。
- **Linux：** 快速开始可用 `sudo apt install gromacs`。正式运行 MD 时，
  建议从源码编译并启用 CUDA 或 ROCm。
- **macOS：** 可用 `brew install gromacs` 安装。Apple 硬件不支持 GROMACS
  GPU 加速，因此 MD 性能会受限。

安装后，打开 SilicoLab，在引擎设置中检测 `gmx` 可执行文件，再运行 MD。

## 远程主机

大型计算也可以通过 SSH 运行在远程 Linux 主机上。当本机没有 GPU，或所需引擎
只在 HPC 登录节点、工作站上可用时，这种方式很有用。

参见[远程执行](./remote-execution/)了解 SSH 配置流程。
