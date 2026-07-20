---
title: 创建第一个项目
description: 使用固定的苯 SMILES 创建、构建、保存并重新打开一个持久化项目。
sidebar:
  order: 4
---

## 目标

从全新的空 **Scratch temporary workspace**（Scratch 临时工作区）开始，创建持久化项目
`SilicoLab Manual Demo`，把固定 SMILES 构建为当前条目 `Benzene`，保存项目，
然后关闭并从最近项目中重新打开。最终可见检查点是 `C6H6`、12 个原子和 12 条键
在重开后仍然存在。

## 固定样例

- 输入：内联 SMILES `c1ccccc1`。它不是 Sample ID，也不需要下载样例文件。
- 起始状态：SilicoLab 已启动，并处于空的 **Scratch temporary workspace**
  （Scratch 临时工作区）；**Entries** 中没有条目，也未打开持久化项目。

## 前置条件

- 完成[安装](../installation/)，并确认图形界面可以启动。
- 阅读[界面导览](../interface-tour/)，能够在 **Entries**（条目）中区分选中的条目与
  当前条目。
- 从全新的空 Scratch 工作区开始。如果 SilicoLab 启动时打开了项目，请选择
  **File > Close Project**；如果 Scratch 中已有条目，请重启 SilicoLab，并在继续前确认
  **Entries** 为空。
- 准备一个可写父目录，且其中尚不存在 `SilicoLab Manual Demo`。本教程不需要 ORCA、
  GROMACS 或其他外部工具。

## 操作

### 1. 创建持久化项目

打开对话框前，确认 **Entries** 为空，且
`<父目录>/SilicoLab Manual Demo` 尚不存在。选择
**File > Create a new project…**（文件 > 新建项目）。在系统存储对话框的
**Save As:**（存储为）中输入 `SilicoLab Manual Demo`，选择该可写父目录并确认。
SilicoLab 会在该父目录下创建同名项目根目录。

如果存储对话框提示名称冲突或询问是否替换现有项目，请选择 **Cancel**（取消），绝不要
确认替换。请选择一个新的空父目录，或改用唯一项目名重新开始，并在后续检查中使用该名称。

**观察：** 标题栏显示 `SilicoLab Manual Demo`，工作区不再是 Scratch，并出现临时状态提示
`Opened project SilicoLab Manual Demo`。

### 2. 打开分子草图窗口

选择 **File > Sketch Molecule…**（文件 > 绘制分子）。

**观察：** **Sketch Molecule**（绘制分子）窗口打开；草图为空时，
**Build (Save as New)**（构建并另存为新条目）处于禁用状态。

### 3. 导入固定 SMILES

在 **SMILES** 输入框中输入 `c1ccccc1`，然后选择 **Import**（导入）。

**观察：** 状态显示 `Imported SMILES (6 atoms)`。这里的 6 个原子是 SMILES
草图中的 6 个碳重原子；构建时才会补入隐式氢。

### 4. 命名并构建条目

在 **Title:**（标题）中输入 `Benzene`，然后选择 **Build (Save as New)**。

**观察：** 新条目自动成为当前条目，并显示在中央视口。状态栏显示
`Benzene | 12 atoms | 12 bonds`；**Details**（详细信息）显示
`Formula: C6H6`；临时状态提示显示
`Built sketched molecule as entry #1 (12 atoms)`。

### 5. 保存项目

选择 **File > Save Project**（文件 > 保存项目）。

**观察：** 临时状态提示显示 `Saved project SilicoLab Manual Demo`。

### 6. 关闭项目

选择 **File > Close Project**（文件 > 关闭项目）。

**观察：** 应用返回 **Scratch temporary workspace**，临时状态提示显示
`Closed project; opened Scratch`。

### 7. 从最近项目重新打开

选择 **File > Recent Projects > SilicoLab Manual Demo**（文件 > 最近项目 >
SilicoLab Manual Demo）。

**观察：** 标题栏恢复为 `SilicoLab Manual Demo`，`Benzene` 恢复为当前条目；
状态栏显示 `Benzene | 12 atoms | 12 bonds`。Details 显示 `Atoms: 12`、
`Bonds: 12` 和 `Formula: C6H6`。另有临时状态提示显示
`Opened project SilicoLab Manual Demo`。

## 预期输出

输出是名为 `SilicoLab Manual Demo` 的持久化项目目录，而不是单个项目文件。
项目数据库记录项目名以及 `Benzene` 条目的标题、12 个原子和 12 条键；本教程不要求
直接打开或编辑这些数据库。关闭项目并重新打开后，以下界面状态应全部恢复：

- `Benzene` 是当前条目，并显示在中央视口；
- Details 显示 12 个原子、12 条键和分子式 `C6H6`；
- 状态栏显示 `Benzene | 12 atoms | 12 bonds`。

## 恢复

| 现象 | 检查 | 恢复方法 |
| --- | --- | --- |
| Scratch 中已有条目 | 本流程需要空的 Scratch 工作区，才能让构建的分子成为 entry #1。 | 不要继续。重启 SilicoLab，必要时关闭自动打开的项目，并确认 **Entries** 为空。 |
| 存储对话框提示名称冲突或询问是否替换 | 所选父目录中已存在同名项目目标。 | 选择 **Cancel**，绝不要确认替换。请选择一个新的空父目录，或改用唯一项目名重新开始，并在后续检查中使用该名称。 |
| 导入后出现 `SMILES error:` | 检查输入是否与 `c1ccccc1` 完全一致。 | 修正输入并再次选择 **Import**。不要用旧草图或空草图执行 Build。 |
| **Build (Save as New)** 仍然禁用 | 检查是否已经成功导入，以及状态是否显示 `Imported SMILES (6 atoms)`。 | 重新输入固定 SMILES 并选择 **Import**；出现成功状态后再 Build。 |
| 系统存储对话框指向错误的父目录 | 在确认前检查对话框当前目录。 | 选择 **Cancel**（取消），重新打开新建项目操作并选择正确的可写父目录；不要通过删除目录来补救。 |
| `Benzene` 行已选中，但中央视口仍显示其他条目 | 检查该行是否只是选中而没有成为当前条目。 | 在 Entries 中双击 `Benzene` 将其激活。 |
| **Recent Projects**（最近项目）中没有该项目 | 确认项目根目录仍然存在。 | 选择 **File > Open Project…**（文件 > 打开项目），并选择 `SilicoLab Manual Demo` 项目根目录。 |

## 科学解释边界

**Build (Save as New)** 会补入隐式氢，使用内置 UFF 从多个初始三维几何尝试松弛，
并保留能量最低的成功结果。这不等于实验结构、完备的构象搜索或研究级方法验证，
也不证明该结果适用于具体科学问题。

## 下一步

- [在快速上手路线图中选择下一页](../quickstart/)
- [仅在工作流需要时配置外部工具](../external-tools/)
