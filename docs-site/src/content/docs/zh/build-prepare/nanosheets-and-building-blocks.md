---
title: 构建纳米片并标注构建块
description: 构建固定的周期性 graphene，并了解 Building Block Authoring 的作用范围。
sidebar:
  order: 7
---

## 目标

从空项目构建默认 `4 x 4 x 1` 周期性 graphene 结构。当前结果必须包含 32 个原子、
48 条已存储键、分子式 `C32`、一个连通分量，以及下方的固定晶胞。

本页还说明 **Building Block Authoring**（构建块编写）。该工具为网状结构工作流标注整个
当前条目；它不会提取所选原子，也不是 Nanosheet Builder 的输入。

## 构建固定 graphene 结构

打开 **Launch**（启动），展开 **Structure Builder**（结构构建），选择
**Nanosheet Builder**（纳米片构建器）。明确输入或确认每个数值：

| 设置 | 数值 |
| --- | --- |
| Name | `Nanosheet` |
| Type | `Honeycomb (A/B)` |
| Preset | `Graphene` |
| Sublattice A / B | `C / C` |
| Lattice a | `2.46 Å` |
| Buckling | `0 Å` |
| Interlayer spacing (A) | `12 Å` |
| Supercell | `4 x 4 x 1` |

对于这个零 buckling 的 graphene 结构，**Interlayer spacing (A)** 表示 c 方向相邻周期像
之间的间距，不是有限片层的厚度。

1. 选择 **Preview**（预览）。检查周期性草稿，不要把它当作已接受的条目。
2. 再次核对所有固定值，然后选择 **Build**（构建）。
3. 确认新的 `Nanosheet` 条目已激活，且 **Activity**（活动）把
   **Nanosheet Builder** 记录为 `Completed`。
4. 检查 Details（详细信息）、结构摘要和已存储连接图。只有下方所有数值一致时才继续。

| 检查项 | 预期值 |
| --- | --- |
| 分子式 / 原子 / 已存储键 | `C32` / 32 / 48 |
| 已存储连接图 | 1 个连通分量 |
| 晶胞长度 | `9.840 x 9.840 x 12.000 Å` |
| 晶胞角 | `90 / 90 / 60°` |
| 已存储电荷 | 每个原子 `0.0` |
| 构建几何的键长 / 键角 | `1.42 Å / 120°` |

只要有任何数值不符，就停止并按固定参数重新构建。视口外观、表示方式和晶胞线框可见性
不会改变已存储数值。

## 理解周期性晶胞

Nanosheet Builder 在三个方向都创建周期性晶胞。对本页零 buckling 的 graphene，
`c = 12 Å` 是相邻周期像之间的间距，不是有限片层的厚度、外边缘或边缘钝化距离。

本页的连通性检查必须使用 `4 x 4` 面内重复。已存储连接图对同一对原子索引只能保存一条键，
因此 `1 x 1` graphene 晶胞无法表示同一原子对经不同周期像形成的三条接触。它可以显示
原胞，但不能用于本页的 48 键、一个分量检查点。

其他 Honeycomb、transition-metal dichalcogenide 和 graphitic carbon nitride 预设不在
本页的定量范围内。生成成功本身不能验证任意元素选择、价态、成键、材料身份或化学适用性；
这些构建中的新原子也以已存储电荷 `0.0` 开始。

## 标注构建块

把准备编写的结构保持为当前条目。打开 **Launch**，展开 **Structure Builder**，选择
**Building Block Editor**（构建块编辑器）。**Building Block Authoring** 面板把整个
当前活动条目作为结构输入。

1. 按需设置 **Label**（标签）和 **Class**（类别）。Class 可以是 **Core**、**Linker**
   或 **Functional group**。
2. 每个 substitution site（取代位点）必须选择两个不同的原子。Leaving atom 必须是与
   binding atom 直接成键的 `Du` dummy atom，binding atom 必须是非 `Du` 原子。
   Selection（选择）只帮助定位这些原子，不会裁切或缩小结构输入。
3. 确认整个当前条目就是目标构建块后，才选择 **Save**（保存）。Save 会把整个条目及
   原子索引未越界的位点标注序列化为 SLF 文件，并写到所选位置。

当前 Save 检查只排除越界的原子索引，不会验证两个原子是否不同、是否分别为 `Du` 与非 `Du`，
也不会验证两者是否直接成键。保存前必须自行确认每个位点。不要把任意两个 graphene carbon
当作 substitution site。SLF 后续用于 reticular 构建时，被 metadata 标记为 leaving 的原子
会从组件中移除；错误的 leaving metadata 因此可能移除错误原子，并产生无效构建块。

保存的文本只注入当前内存中的 reticular 草稿，不会注册供其他项目或后续重启使用的持久
组件库。Nanosheet Builder 不会读取该构建块；保存它也不会改变 graphene 草稿。

## 恢复到已知状态

| 现象 | 恢复方法 | 继续前的检查 |
| --- | --- | --- |
| graphene 参数有变 | 恢复 Honeycomb Graphene、C/C、`2.46 Å`、`0 Å`、`12 Å` 和 `4 x 4 x 1`，再重新预览 | 所有固定输入一致 |
| 无法确定预览状态 | 在 Build 前选择 **Cancel** 并重新打开构建器 | 尚未接受任何结果条目 |
| 构建了 `1 x 1` 或其他错误片层 | 在干净的可写项目中重新开始 | 新条目符合所有固定 graphene 检查项 |
| 超胞扩展改变了当前结果 | 重新构建未扩展的 graphene，不要反推先前状态 | 恢复 32 个原子、48 条键和原始晶胞 |
| 为错误条目打开了 Building Block Authoring | 在 **Save** 前选择 **Cancel**；如果已经保存文件，也不要把它用作 nanosheet 输入 | nanosheet 保持不变 |
| 不存在有效的 `Du`-binding bond | 选择 **Cancel**；不要保存或导入 SLF。先建立正确的 dummy-site model，或改用合适的 building block | 每个位点都有不同的 `Du` leaving atom，并与非 `Du` binding atom 直接成键 |

只要分子式、计数、晶胞、连通性、成键几何或当前条目身份不确定，就应停止。不要从扩展过或
参数错误的结果继续。

## 科学限制

该 graphene 结果是周期性的，没有有限边缘、边缘终止、钝化或缺陷模型。`12 Å` 间距不能
证明周期像之间的相互作用可以忽略。构建器不会分配经过验证的部分电荷或力场参数，不会最小化
几何、优化堆叠，也不会评估声子或动力学稳定性。预设名、`0.0` 电荷、`1.42 Å` 键长和
`120°` 键角本身不能证明稳定性、可合成性或可直接用于模拟。进入后续使用前，应验证模型并
执行所需松弛。

## 相关页面

- [构建网状结构](../reticular-structures/)
- [使用周期性晶胞与超胞](../periodic-cells-and-supercells/)
- [编辑与导出结构](../../projects-structures/edit-and-export/)
