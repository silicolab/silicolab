---
title: 构建无序起始几何结构
description: 装填可复现的周期性 argon 示例，并判断完整、部分、停止和超时结果。
sidebar:
  order: 5
---

## 目标

配置一个确定性请求，把单原子 argon 条目的 8 个刚性副本装填到 `16 x 16 x 16 Å`
立方体中，spacing 为 `2.0 Å`，seed 为 `3`。检查已存储的输出结构和晶胞，并理解当前 GUI
会保留和不会保留哪些装填字段。

## 固定样例与请求

下载 [`BP-ARGON-01`](../../../samples/argon.xyz)。该 fixture 包含 1 个 Ar 原子、0 条键，
并且没有晶胞。它不包含力场参数、热力学状态或实验来源。

把 `argon.xyz` 导入新项目并明确激活。在 **Launch**（启动）中展开
**Molecular Dynamics**（分子动力学），选择 **Disordered System**（无序系统）。

| 设置 | 数值 |
| --- | --- |
| Result name | `Periodic argon` |
| Specify amount by | `Copies` |
| Molecule / amount | `BP-ARGON-01` / `8 copies` |
| Region / size | `Box` / `16 x 16 x 16 Å` |
| Result cell | 启用 **Use the region as the result's simulation cell** |
| Boundary scoring | 启用 **Pack periodically (no clashes across box edges)** |
| Spacing / seed | `2.0 Å` / `3` |
| Pack around | `None` |
| Advanced | `Max restarts = 20`；`Max steps = 2000` |

设置 seed 后不要选择 **Randomize**。

## 构建并检查结果

| 步骤 | 操作 | 可观察结果 |
| --- | --- | --- |
| 1 | 导入并激活 `BP-ARGON-01` | Details（详细信息）显示 1 个 Ar 原子、0 条键和无晶胞 |
| 2 | 打开 **Disordered System**，添加当前条目并输入固定请求 | 面板在 `Copies` 模式下显示 1 个 component row 和 8 个请求副本 |
| 3 | 选择 **Build**（构建） | 名为 `Periodic argon` 的结果条目立即创建；**Activity**（活动）显示装填任务正在运行 |
| 4 | 如果出现 live progress line，可在运行期间查看 | 进度约每 `75 ms` 更新一次，显示 placed/requested、steps 和保留 2 位小数的 worst overlap；固定 8-Ar 运行可能在任何 line 出现前直接完成 |
| 5 | 运行结束后检查 Details 和 **Activity**；如果成功提示仍可见也一并检查 | 提示为 `Packed 8 molecules into a disordered system`；**Activity** 把 **Build Disordered System** 记录为 `Completed`；Details 显示下方已存储检查项 |

当前 GUI 在任务结束后不保留详细 `PackReport`。**Activity** 只保留 `Completed`，临时成功提示
也只给出 packed count。没有出现 live line 不表示失败，重新运行也不能保证它出现。

| 检查项 | 预期可见状态 |
| --- | --- |
| 可选的 live progress（若出现） | placed/requested、steps 和舍入到 2 位小数的 worst overlap；它可能显示 `8/8 placed` 和 `0.00 Å` |
| 仍可见时的临时成功提示 | `Packed 8 molecules into a disordered system` |
| Activity | **Build Disordered System** 为 `Completed`；这只证明工作流已经结束 |
| 结果名 / 分子式 | `Periodic argon` / `Ar8` |
| 原子 / 已存储键 | 8 / 0 |
| 已存储连接图 | 8 个连通分量 |
| 晶胞 | `16.000 x 16.000 x 16.000 Å`；`90.000 / 90.000 / 90.000°` |
| 区域体积 | `16 x 16 x 16 = 4096 Å³` |

这些已存储检查项可以确认输出组成和晶胞。8 个原子来自 `1 atom per copy x 8 copies`，
但原子数和 `Completed` 都不能证明全部副本已放置或结构无冲突。

## 引擎参考值（当前 GUI 不持久保留）

| 引擎字段 | 确定性参考值 |
| --- | --- |
| Requested / placed / unplaced | `8 / 8 / 0` |
| Convergence | `converged = true` |
| 最大残余重叠 | `0.000000 Å` |

这些精确值是固定请求的确定性引擎参考，不是 GUI 强制验收门槛。当前 GUI 无法在完成后
证明这些报告字段；它只会在 live progress 出现时显示保留 2 位小数的 overlap，之后不保留
详细 `PackReport`。

## 数量模式与混合物

| Amount mode | 每个 component row 的换算方式 |
| --- | --- |
| **Copies** | 使用该行输入的整数副本数 |
| **Density (g/cm^3)** | 使用该行模板质量和完整区域体积换算该行密度 |
| **Concentration (mol/L)** | 使用完整区域体积换算该行摩尔浓度 |

所选模式应用于所有行，但每一行都依据完整区域体积独立换算。多个行不会自动归一化为总密度、
摩尔分数或混合比例，工作流也不检查总电荷。要控制混合组成，应先确定每个组分的整数副本数，
再用 **Copies** 输入。

## 周期评分与结果晶胞

**Pack periodically** 使用最小镜像边界评分计算盒子相对表面之间的 spacing penalty。
对于 `Box`，启用 periodic packing 还会使引擎把 Box 晶胞写入结果，即使显式 output-cell
复选框关闭。**Use the region as the result's simulation cell** 会发出显式 output-cell 请求。
本示例同时启用两个选项，使边界评分和输出晶胞意图都清楚可见。

每个 component 都作为刚体平移和旋转，并保留内部坐标和键。Spacing 是不同刚体之间
与元素无关的距离阈值。Packing penalty 不是力场、势能或统计系综。

## 完整、部分、停止和超时结果

- 达到 step 或 restart 上限的运行可能以 unconverged 结束，运行可能 timed out，按 Esc
  会请求停止并保留当时已装填的结构。当前 GUI 无法仅从完成后的 **Activity** 和 Details
  追溯区分所有这些报告状态。
- 如果界面明确报告 stopped、timed out、partial 或 packed copies 少于 8，不要把该条目用作
  固定参考。再次运行前先修正所有请求错误。
- 在任何下游模拟前，导出结构或使用合适的分析工具，独立检查组成、晶胞、分子完整性和每个
  周期最小镜像距离。
- 如需正式可审计的 placed/unplaced/converged 报告，应等待产品支持持久化 `PackReport`，
  或使用能输出该报告的验证路径。不要从 GUI 状态推断。
- 在 **Build** 前选择 **Cancel**（取消）只丢弃面板草稿，不会产生装填结果。

## 恢复到已知状态

| 现象 | 恢复方法 | 继续前的检查 |
| --- | --- | --- |
| 输入不是单原子 Ar 或选错 component | 取消面板，重新导入 fixture，并激活全新条目 | Details 显示 1 个 Ar 原子、0 条键和无晶胞 |
| 构建前有字段错误 | 选择 **Cancel**，重新打开面板，并恢复全部固定值 | 1 行、8 copies、16 Å 立方体、两个选项、2.0 Å、seed 3 和无 obstacle 一致 |
| 用错误字段启动了装填 | 按 Esc，只保留结果用于诊断，再从全新输入开始 | 只有按固定请求生成的新结果才符合条件 |
| 没有出现 live progress line | 不需要仅因此进行恢复；检查 Activity 和已存储输出 | 记住该 line 是可选的，另一次运行也可能不显示 |
| 运行明确为 partial、stopped 或 timed out，或提示 packed molecules 少于 8 | 只保留条目用于诊断；如有需要，修正请求后再运行 | 不要从保留结构推断缺失的报告字段 |
| 已存储分子式、计数、分量或立方晶胞不同 | 同时启用 periodic 和 result-cell 选项后重新运行 | Details 显示 `Ar8`、8 个原子、0 条键、8 个分量、3 条 16.000 Å 长度和 3 个 90.000° 角 |
| 需要精确收敛或放置分类 | 使用能输出可审计报告的验证路径，或等待 GUI 持久化 `PackReport` | 不要用 Activity、提示或原子数替代 |

不要只根据条目存在、原子数、临时提示或 Activity 状态推断无冲突或完整放置。

## 科学限制

该运行只检查一个固定请求和 seed 的刚体 spacing 优化。它不会分配力场、计算能量、
最小化分子内几何、平衡体系或采样系综。`converged` 和 `0.000000 Å` 只描述该引擎的
确定性参考 packing penalty 和残余重叠指标；可选 live line 会把 overlap 舍入到 2 位小数。
这些数值都不能证明物理密度、原子半径、一般意义上的无冲突或模拟稳定性。

进行动力学或统计采样前，应独立检查组成、电荷、晶胞、每个周期最小镜像距离、分子完整性、
拓扑和力场参数，再执行合适的最小化与平衡方案。

## 相关页面

- [使用周期性晶胞与超胞](../periodic-cells-and-supercells/)
- [导入、获取与绘制结构](../../projects-structures/import-fetch-sketch/)
- [编辑与导出结构](../../projects-structures/edit-and-export/)
