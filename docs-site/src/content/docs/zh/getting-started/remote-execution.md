---
title: 远程执行
description: 通过 SSH 将大型计算任务提交到远程 Linux 主机。
sidebar:
  order: 4
---

大型计算可以转移到远程 Linux 主机上运行，例如 HPC 登录节点或 GPU 工作站；
图形界面仍然留在你的笔记本上。

量子化学、分子对接和 MD/GROMACS 都可以远程运行。SilicoLab 会调用操作系统
自带的 OpenSSH 客户端（`ssh` 和 `scp`）。macOS 和 Linux 默认提供 OpenSSH
客户端；Windows 11 如缺少该组件，可在 **设置 > 应用 > 可选功能 >
OpenSSH Client** 中启用。

首次使用时，SilicoLab 会向主机部署一个小型自包含 worker。worker 会绑定到
当前应用版本，并在运行前用发布的校验和验证。

如需从源码测试远程执行变更，请参阅
[远程执行开发指南](https://github.com/silicolab/silicolab/blob/main/docs/developing-remote-execution.md)。
正式发布的版本始终使用与应用版本绑定并经过校验和验证的 worker。

## 配置主机

打开 **Settings > Engines > Remote Hosts**。

1. 选择 **Add host**，填写标签、主机名或 IP、用户名，并可选填写端口和远程
   工作目录。默认工作目录是 `~/.silicolab`。自定义目录必须是 Linux 绝对路径，
   或以 `~/` 开头。
2. 在 **Setup commands** 中填写非交互 SSH shell 启动后需要执行的命令，让
   引擎命令可用。例如可用 `module load gromacs` 或
   `source /opt/gromacs/bin/GMXRC` 让 `gmx` 可运行。每行填写一条命令。
3. 选择 **Set up passwordless login**。SilicoLab 会生成一把专用密钥：
   `~/.silicolab/keys/id_silicolab_ed25519`，并显示一条需要在远程主机上
   执行一次的命令。这把密钥独立于你的个人 SSH 密钥。
4. 选择 **Verify**，确认基于密钥的登录可用。无密码登录是必需的，否则无人值守
   任务可能会卡在密码提示上。
5. 如果要运行 MD，选择 **Detect GROMACS**。这会在主机上探测 `gmx` 并记录
   版本。量子化学和分子对接运行在部署的 worker 内，因此不需要主机侧安装
   GROMACS。

## 远程运行

**Run MD**、**Build MD System**、**QM** 和 **Molecular Docking** 面板都有
**Run on** 选择器，可在其中选择远程主机。在 **Build MD System** 中，该选择器
只作用于 GROMACS 构建步骤；内置几何构建始终在本地运行。

新面板会使用 **Settings > Engines > Remote Hosts** 中配置的
**Default compute target**，但每次运行前都可以单独修改。

SilicoLab 会上传输入文件、运行任务、回传实时日志并下载结果。结果会像本地运行
一样出现在项目中。对于 GROMACS 任务，每个 `gmx` 步骤都会以 detached 方式启动，
因此 SSH 连接中断不会杀掉计算。

按 **Esc** 可以取消远程任务；SilicoLab 也会停止远程作业。

## 当前限制

- 远程任务运行时会占用唯一的 engine-job 槽位。
- 关闭应用后，正在运行的远程任务会继续留在主机上。SilicoLab 会在本地运行目录
  写入 `remote_run.json` 记录，但目前还不会自动重新连接该任务。
- `<work_root>/runs/<run-id>` 下的远程临时目录目前不会自动清理。
