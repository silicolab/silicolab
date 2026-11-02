---
title: 装填有机分子的 MD 体系
description: 用 acpype 为每种分子分型，把链和客体装填进盒子，再由 md build 拼出 GROMACS 的 GAFF2 拓扑。
sidebar:
  order: 10
---

## 目标

对没有生物分子残基模板的本体有机体系（聚合物链、富勒烯、溶剂或吸附质分子，任意组合）运行 GROMACS。力场工作对每种分子只做一次；之后盒子可以随意重新装填、重建。

## 1. 为每种分子分型

`forcefield type` 把一个分子交给 acpype（见[外部工具](../getting-started/external-tools/)），并以给定名称把分型后的拓扑存入力场库。盒子里将出现的每一种分子都要分型，而且要用将被装填的那组原子：封端的低聚物而不是裸重复单元，客体分子就按草绘的那样。

```text
sketch "CC(c1ccccc1)CC(c1ccccc1)CC(c1ccccc1)C" --name ps3
forcefield type ps3 --name ps3
sketch c1ccccc1 --name benzene
forcefield type benzene --name benzene
```

几十个原子的 AM1-BCC 电荷需要几秒到几分钟，且随分子增大急剧变慢，所以要对带封端的短寡聚物分型，而不是整条链。`forcefield list` 列出库中内容。

## 2. 装填盒子

`disorder`（别名 `pack`）把若干条目的拷贝放进一个晶胞，并把每个拷贝记录为一个残基；残基名是条目名前三个字母的大写：

```text
pack --of ps3 --count 8 --of benzene --count 20 --box 32,32,32 --seed 1
```

用 `--density` 代替 `--count` 可按质量密度确定链数。较低的起始密度（0.1–0.3 g/cm³）装填得快；运行中的压缩阶段会把它带到真实密度。

## 3. 构建并运行

```text
md build
md simulate --time 500ps --temperature 300
```

`md build` 找出活动结构中的分子（键的连通分量），把每个分子匹配到库中原子相同、顺序相同且原子间成键也相同的条目（元素顺序相同的异构体会被拒绝），并捕获 GAFF2 拓扑：带 AM1-BCC 电荷的分型原子、带 acpype 显式参数的键合项，以及按坐标顺序排列的分子计数。回复会列出组成，例如 `… atoms as 8× ps3, 20× benzene`。

当两个库条目原子相同（同一条链的两个构象分别分型，或同一分子分别用 `gaff` 和 `gaff2` 分型）时，按残基指定用哪一个：

```text
md build --typed PS3=ps3,BEN=benzene
```

之后走常规的 `md simulate` / `md run` 路径，本机或远程主机均可。

## 注意

- 盒子里的每个分子都必须在库中有匹配；没有匹配的分子会终止构建并报出其位置和分子式。单原子离子尚未覆盖。
- 运行使用 GAFF2 拟合时采用的 AMBER 非键处理：PME 静电、potential-shift Lennard-Jones 和色散校正。
- 拓扑的质量取决于分型：一条低聚物分型一次后被所有拷贝复用，因此端基带的是封端电荷。
