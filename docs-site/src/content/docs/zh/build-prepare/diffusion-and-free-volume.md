---
title: 致密相中的扩散与自由体积
description: 在 Console 中测量装填盒子的探针可及自由体积、吸附质分子的自扩散系数、Widom 插入法吸附量以及径向分布函数。
sidebar:
  order: 9
---

## 目标

对一个装填或模拟过的盒子（含吸附客体的聚合物膜、液体、玻璃态体系），得到传质论证所依赖的两个数：给定尺寸的探针能到达多少自由体积，以及每种吸附质分子移动得多快。

## 自由体积

`membrane free-volume` 在周期晶胞上铺一层格点，统计没有被任何原子的范德华球（半径加上探针半径）覆盖的格点。活动条目必须带有晶胞：来自 `disorder`、`md build` 或一次 MD 运行的盒子。

```text
pack --of active --count 20 --box 18,18,18
membrane free-volume --probe 1.0
```

回复形如 `free volume 30.7% of 5832 Å³ (1792 Å³) with a 1.0 Å probe on a 0.50 Å grid; density 0.445 g/cm³`。选项：

| 选项 | 含义 |
| --- | --- |
| `--probe <Å>` | 探针半径（默认 1.0 Å）。`0` 给出纯粹的未占据分数；1.3 Å 模拟 PALS 实验中的正电子素探针。 |
| `--grid <Å>` | 格点间距（默认 0.5 Å）。减半会使开销增至约八倍，精度约提高一个百分点。 |
| `--trajectory <path> --frames N` | 对轨迹最后 `N` 帧取平均（使用每帧各自的盒子），而不是用条目自身的坐标。只给 `--frames` 时使用条目记录的 MD 轨迹。 |

这个分数是纯几何量：只取决于 Bondi 半径和探针，与力场无关，其意义取决于装填质量。直接从 `disorder` 出来的盒子远远过稀；在 `md simulate` 压缩并平衡体系之后，这个数才有意义。

## 扩散

`membrane diffusion` 在轨迹中跟踪每个选中分子的质心，对所有时间原点计算均方位移（MSD），并在滞后时间范围的中段拟合 Einstein 关系 `MSD = 6 D t`。

```text
membrane diffusion --select BEN --out msd.csv
```

不给路径时读取条目记录的 MD 轨迹；也可以传入同一拓扑的另一次运行的 `.xtc`、`.trj` 或 `.xyz` 文件。回复形如 `D = 7.9e-6 cm²/s for 20 molecules (`BEN`) over 60 frames of md.xtc; MSD exponent 1.12 on 14–48 ps`。

| 选项 | 含义 |
| --- | --- |
| `--select` | `all`、元素符号或残基名（`disorder` 装填的苯拷贝是 `BEN`）。选区内相互成键的原子构成一个分子。 |
| `--max-lag <ps>` | 评估的最长滞后时间（默认：轨迹长度的一半）。 |
| `--fit from,to` | 用于拟合的滞后范围分数（默认 `0.2,0.8`）。 |
| `--out <csv>` | 写出 MSD 曲线（`lag_ps,msd_angstrom2`）以便绘图。 |

坐标会按每帧的盒子（XYZ 轨迹则用条目的晶胞）解除周期折叠，所以 GROMACS 折叠过的 `.xtc` 输出不需要预处理。注意指数：MSD 在拟合窗口内的双对数斜率为 1 表示 Fickian 扩散；明显小于 1 说明吸附质仍困在孔穴中、运行太短，命令会提示这一点。XYZ 轨迹的每帧注释行需要带 `t=` 时间（ps）；没有时按帧间隔 0.005 ps 处理，回复会提醒 D 依赖于这个假设的间隔。帧时间必须递增且等间隔：由续跑拼接而带重复帧、或中间缺帧的轨迹会直接报错，而不是按错误的时间轴拟合。

渗透选择性由两次运行得出：同一膜中两种吸附质扩散系数之比，乘以它们的吸附量之比。吸附那一半由 `membrane sorption` 提供。

## 吸附

`membrane sorption` 执行 Widom 试探粒子插入：把探针分子的一个拷贝以随机位置和取向放入主体盒子，计算它与主体的相互作用能，Boltzmann 平均 `⟨exp(−ΔU/kT)⟩` 给出过量化学势 `μ_ex = −kT ln⟨…⟩` 以及无限稀释溶解度系数 `S = exp(−μ_ex/kT) / (R T)`，单位 cm³(STP)/(cm³·cmHg)。

```text
forcefield type benzene --name benzene
membrane sorption benzene --frames 20
```

主体是活动条目的已分型 MD 体系（由力场库中分型的分子 `md build` 而来），探针是另一个条目，其分子也在力场库中分型，因此二者都带有 Lennard-Jones 参数和部分电荷。回复形如 `benzene: μ_ex = -14.2 kJ/mol, ⟨exp(−ΔU/kT)⟩ = 2.96e2 ± 4.1e1, S = 3.550e0 cm³(STP)/(cm³·cmHg) at 300 K from 400000 insertions over the last 20 frames of md.xtc (22.4% favourable)`。

| 选项 | 含义 |
| --- | --- |
| `--entry` | 主体条目（默认 `active`）。 |
| `--temperature <K>` | 插入温度（默认 300 K）。 |
| `--insertions N` | 每帧试探插入次数（默认 20000）。给出的误差是 Boltzmann 平均值的标准误。 |
| `--cutoff <Å>` | 对截断（默认 10 Å）；盒子至少要有两倍宽。截断之外的 Lennard-Jones 按均匀主体的解析长程尾修正补上；库仑项采用 shifted-force 形式，求和结果不依赖于分子中哪些原子恰好跨在截断球面上。 |
| `--seed N` | 随机种子；相同种子重现相同的插入序列。 |
| `--trajectory <path> --frames N` | 对最后 `N` 帧取平均，而不是用条目自身的坐标。只给 `--frames` 时使用条目记录的 MD 轨迹。 |

插入法便宜但盲目：在致密玻璃态聚合物中多数试探落在原子内部，只有找到孔穴的少数试探才有贡献，所以要多取帧，并预期误差棒收缩缓慢。比自由体积单元还大的探针完全插不进去，Boltzmann 平均趋于零，此时这个数说明的是方法而非材料。溶解选择性是同一批帧中两种探针 `S` 之比。

## 径向分布

`membrane rdf` 对两个选区之间的最小镜像距离做直方图，并按理想气体期望归一化，得到 `g(r)` 和累积配位数。

```text
membrane rdf BEN C --frames 20 --out rdf.csv
membrane rdf O
```

只给一个选区时统计其内部的原子对。回复给出第一个峰；`--out` 写出 `r_angstrom,g,coordination`。

| 选项 | 含义 |
| --- | --- |
| `--entry` | 主体条目（默认 `active`）。 |
| `--rmax <Å>` | 统计的最长间距（默认 12 Å）；不得超过盒子的一半。 |
| `--bin <Å>` | 分箱宽度（默认 0.1 Å）。 |
| `--trajectory <path> --frames N` | 同上，对末尾若干帧取平均。 |
