---
title: 外部工具
description: 配置 SilicoLab 各功能模块调用的外部程序。
sidebar:
  order: 5
---

SilicoLab 在没有可选外部工具时也可以启动。只有在使用对应功能时，才需要安装
这些程序。

## ORCA

量子化学计算默认使用内置 Hartree 引擎。ORCA 是可选引擎，首版支持分子体系的
单点能、几何优化和振动频率计算；过渡态与周期 QM 计算仅支持 Hartree。

请单独安装 ORCA，然后打开 **Settings > Compute > Compute targets**，在相应目标的
ORCA 行中填写 **Program**，再选择 **Verify**。SilicoLab 不会搜索 ORCA。在
Windows 中使用 WSL 版 ORCA 时，将命令前缀设为 `wsl.exe -e`，程序填写 WSL 内的
通用 Linux 路径，例如 `/opt/orca/orca`；原生安装则将命令前缀留空。

在 QM 任务面板中显式选择 ORCA，或在脚本中使用 `qm energy --engine orca`。
ORCA 默认使用一个 CPU 核心；请求更多核心会启用 `%pal` 并行模式，并要求目标
环境中能够调用 `mpirun`。

## xtb

[xtb](https://github.com/grimme-lab/xtb) 提供 GFN2-xTB、GFN1-xTB 和 GFN-FF
半经验方法，支持分子体系的单点能、几何优化、频率计算和反应性分子动力学
（`qm md`）。对过渡金属团簇这类 DFT 要算数小时的体系，GFN-xTB 只需数秒，但精度
有限，能量和轨迹应视为定性结果。

可用 `conda install -c conda-forge xtb` 安装，或下载发布版二进制文件。SilicoLab
会在 `PATH` 和常见的 Homebrew 路径中查找 `xtb`。若安装在其他位置（例如 conda
环境中，从 Finder 启动的应用看不到），请在 **Settings > Compute > Compute targets**
的 xtb 行中填写 **Program**，再选择 **Verify**。

在 QM 任务面板中选择 xtb，或在脚本中运行 `qm md --temp 300 --time 2`（`qm md`
默认使用 xtb）。计算结束后会自动播放生成的多帧 XYZ 轨迹，并逐帧重新判断成键。

## acpype（AmberTools）

[acpype](https://github.com/alanwilter/acpype) 封装了 AmberTools 的
`antechamber`，为单个有机分子分配 GAFF2 原子类型和 AM1-BCC 电荷，并输出
GROMACS 拓扑。SilicoLab 通过 Console 命令 `forcefield type`（别名 `ff type`）
调用它，用于内置 MD 表无法分型的客体分子、溶剂和封端低聚物。

用 `conda install -c conda-forge acpype` 安装（会一并安装 AmberTools）。
SilicoLab 会在 `PATH` 和常见的 Homebrew 位置查找 `acpype`；如果装在别处，
请在 **Settings > Compute > Compute targets** 的 acpype 行中填写 **Program**
并选择 **Verify**。

```text
forcefield type active --name benzene
forcefield list
```

该命令在前台运行（AM1-BCC 对溶剂分子只需数秒，对 200 原子的低聚物需要数分钟；
`--timeout` 以分钟为单位设置上限），并把 acpype 生成的 `.itp` 以给定名称保存到
力场库（`~/.silicolab/force_fields/`）中。该拓扑是完整的 GROMACS
`[ moleculetype ]`，`md build` 目前还不会读取它；可直接用于 GROMACS 运行，
或等待后续的有机分子成键拓扑功能来拼接它。净电荷默认取结构形式电荷之和；
缺少形式电荷时用 `--charge` 指定。`--method gas` 切换到更快但更粗糙的 Gasteiger
电荷，`--atom-types gaff` 切换到第一代 GAFF 类型集。GAFF 只覆盖 H、C、N、O、F、
P、S、Cl、Br、I，且分子必须带有键。

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

安装后，打开 **Settings > Compute > Compute targets**，展开准备运行任务的目标。
在该目标的 GROMACS 行中填写 **Program**；也可以将 **Program** 留空，让
**Verify** 在该目标上搜索。运行 MD 前请选择 **Verify**。

## 远程主机

大型计算也可以通过 SSH 运行在远程 Linux 主机上。当本机没有 GPU，或所需引擎
只在 HPC 登录节点、工作站上可用时，这种方式很有用。

参见[远程执行](../remote-execution/)了解 SSH 配置流程。
