---
title: 远程执行
description: 通过 Direct SSH 或 Slurm 在远程 Linux 主机上运行计算任务。
sidebar:
  order: 4
---

量子化学、分子对接和 MD/GROMACS 任务可以在远程 x86-64 Linux 主机上运行，
图形界面仍保留在本机。SilicoLab 使用系统的 `ssh` 和 `scp`，首次使用时会部署
与应用版本匹配并经过校验的 worker。

## 配置 SSH

打开 **Settings > Engines > Remote Hosts**，填写主机地址、SSH 用户、端口和工作
目录，然后选择 **Set up passwordless login**。SilicoLab 会在
`~/.silicolab/keys` 下创建专用密钥，并保持严格的主机密钥验证。只需在远程主机
执行一次界面显示的授权命令，再选择 **Verify**。

工作目录默认为 `~/.silicolab`。该目录必须能被所有可能执行任务的计算节点读写；
集群通常应使用共享 home、项目目录或 scratch 文件系统。配置 Slurm 后选择
**Test scheduler**，SilicoLab 会提交一个真实的短任务，验证计算节点能看到 worker。

**Job environment commands** 在分配到的作业内部、worker 启动前执行，可用于
`module load gromacs` 或 CUDA 环境。**Scheduler setup commands** 在登录节点调用
`sbatch`、`squeue`、`scontrol` 和 `scancel` 前执行，仅在非交互 SSH 环境找不到
Slurm 命令时填写。

## Direct SSH

专用工作站或裸计算节点请选择 **Direct SSH**。worker 会作为分离的进程组运行。
CPU 请求会限制 worker 线程池；由于没有调度器，内存和时限不会被强制执行。

## Slurm

选择 **Slurm**，并按集群要求配置：

- **Partition**：队列，例如 `debug` 或 `gpu`；
- **Account**：作业计费使用的分配账户；
- **QOS**：集群定义的服务等级；
- **Reservation** 和 **Constraint**：可选的高级筛选条件；
- 默认 CPU、内存和时限：任务未单独覆盖时使用。

选择 **Detect Slurm** 验证 `sbatch`、`squeue`、`scancel` 和 `scontrol`。
`sacct` 不是必需项；不可用时 SilicoLab 会用 `scontrol` 查询终态。选择
**Refresh cluster** 可获取分区、GPU 类型和节点特性建议，这些信息只是提示，
不代表资源已经预留。

GPU 默认使用 GRES 语法。每个任务可以选择：

- **No GPU**；
- **Any available GPU** 并填写数量；
- **Specific type**，填写 `a100` 等集群 GPU 类型及数量。

SilicoLab 会将其转换为 `--gres=gpu[:type]:count`。只有集群管理员明确要求时才改用
`--gpus`。两者都不适用的集群可以选择 **Custom**，填写含 `{count}` 占位符（`{type}`
可选）的模板参数，例如 `--gres=accel:{type}:{count}`。物理 GPU 编号由 Slurm 分配，
并通过作业环境暴露给引擎。

## 运行与监控

在任务面板的 **Run on** 中选择主机，然后设置 CPU、内存、时限和 GPU 意图。
任务监视器会显示 queued、running、completing、cancelling 及终态。Slurm 排队任务
还会显示 `Priority`、`Resources`、`InvalidAccount` 或 `InvalidQOS` 等原因。

选择 **Refresh Remote** 可更新状态并只获取新增日志。关闭 SilicoLab 不会停止远程
任务；重新打开项目后刷新即可继续监控或取回结果。提交时保存的调度器和远程目录
始终有效，之后修改主机配置不会改变已有任务的位置。

对排队中或运行中的任务选择 **Cancel**。Slurm 作业会保持 **Cancelling**，直到
调度器确认 `CANCELLED`；重复取消是安全的。只有确认终态后才能删除远程临时目录。

对于 Slurm 目标，登录节点的 CPU/GPU 利用率不会被显示为集群利用率；请在任务
监视器查看分配状态和排队原因。

## 故障排查

- **Account、QOS 或 Partition 无效**：使用管理员提供的精确值，并刷新查看 Slurm
  排队原因。
- **计算节点看不到 worker**：将工作目录改到共享文件系统，再运行
  **Test scheduler**。
- **没有终态历史**：集群可能禁用了 `sacct`。SilicoLab 会自动回退到 `scontrol`，
  但可查询时长仍取决于控制器的保留策略。
- **指定 GPU 类型后一直排队**：用 **Refresh cluster** 检查类型拼写，并确认分区
  包含该 GRES 类型。
- **找不到 GROMACS**：在 **Job environment commands** 中加载相应模块或环境，
  然后运行 **Detect GROMACS**。
