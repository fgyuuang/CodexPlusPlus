# Codex++ 空间与开发路径审计（2026-11-16）

后续状态：用户追加要求后，当前主仓库已直接持有 .git，历史统一收纳在 D:/pro/codex-plus-plus/history，并仅保留一个登记中的开发工作区。下文为本阶段快照；最新目录、占用及未执行删除的边界见 [历史收敛报告](2026-11-16-history-consolidation.md)。

本次仅清理可重建编译产物并调查本地历史，没有合并、提交、推送或启动产品构建。统计跳过 Junction/重解析点，旧路径链接不重复计算。GiB = 1024³ 字节。

## 已执行的清理

| 工作区 | 开发编译 | Release 编译 |
| --- | ---: | ---: |
| 原 codexpp-v266-integration-20261112 | 57.4 GiB | 7.9 GiB |
| 当前 codex-plus-plus | 19.5 GiB | 6.6 GiB |

四次操作均先用 Cargo 按明确 target-dir 和 profile 预览，再按同一目标清理，全部成功。未对整个仓库递归删除。

| 范围 | 清理前逻辑大小 | 清理后逻辑大小 |
| --- | ---: | ---: |
| D:/pro/codex-plus-plus | 27.56 GiB | 1.43 GiB |
| D:/pro/codex-plus-plus-history | 91.60 GiB | 26.25 GiB |
| D:/pro/Codex++（安装与备份） | 6.15 GiB | 6.15 GiB |
| 两个本地插件开发目录 | 0.04 GiB | 0.04 GiB |
| 合计 | **125.35 GiB** | **33.87 GiB** |

D 盘实际可用空间从 **103.98 GiB 增至 187.94 GiB**，前后差额约 **83.96 GiB**。文件逻辑长度不等于实际分配空间；未对所有 NTFS 文件标识去重，空间快照也可能受其他进程写入影响，因此不能将逻辑删除的 91.49 GiB 直接当成磁盘回收量。

D 盘总容量约 1907.71 GiB，清理后全盘已用约 1719.77 GiB。Codex++ 是明显的占用来源，但清理前相关目录的逻辑大小约为当时全盘已用空间的 7%，不足以把整个 D 盘占用都归因于它。本次没有审计或删除其他软件与私人文件。

## 剩余材料与清理限制

- 当前开发区约 1.43 GiB，保留源码、当前 bin 和最新成功交付。
- 历史目录约 26.25 GiB：迁移材料约 12.79 GiB，旧 C 基线约 10.41 GiB，其他工作区约 3.05 GiB。旧 C 基线中的 output 约 6.84 GiB；这些子项相互包含，不可重复相加。
- 迁移材料中，verified-copies 约 10.76 GiB，git-before 约 2.02 GiB；它们是复制备份，不能当成另一条正在开发的路线。
- 安装目录约 6.15 GiB，其中历次备份约 5.56 GiB。当前实际运行文件位于 D:/pro/Codex++。
- 当前公共 Git 仍在 D:/pro/codex-plus-plus-history/c-baseline-2.4.0/.git。不可整删 history 或该 .git；当前主开发目录依赖它。
- 历史源码仍有独有未提交修改、补丁和未结束的合并，不能仅按版本号直接删除。

已生成 historical-binary-clean-plan.json：覆盖约 9.88 GiB 的旧构建与安装备份二进制（2536 个文件），排除当前安装及最新成功任务 20261116-165859-52005205。自动审批拒绝了按清单批量删除的操作，返回 **blocked by policy**，没有提供更具体原因；整项操作未执行，也没有换用其他工具重试。迁移副本去重仅审计，未执行删除。

迁移副本已存在上一阶段逐文件复制校验的成功证据。此次额外启动的重复文件完整哈希复核在删除受阻后停止，未完成，未生成可执行的去重删除计划；不得把该扫描称为全量复核通过。停止记录见 duplicate-verification-stopped.json。

因此本次已完成最大项的编译清理，尚未达到“只剩核心”的状态。旧发布记录的 state 保留原样；没有执行已列出的旧二进制删除，不存在将缺失封存继续宣称可重新交付的问题。

## 何时开始使用 D 盘，谁执行了创建

| 北京时间 | 证据与事件 |
| --- | --- |
| 2026-06-05 | D:/pro/Codex++ 安装目录创建时间。它只能说明安装目录，不能证明源码当时已迁移。 |
| **2026-11-12 10:44:10** | Git reflog 首条记录创建 codexpp-v266-integration-20261112。context.json 在 10:44:12 记录 C 盘备份路径与 D 盘新工作区。Codex Desktop 会话保存了 git worktree add -b codex/v266-extension-migration 的工具调用。 |
| **2026-11-14 16:30:09** | Git reflog 首条记录创建 codexpp-v269-review-20261114。Codex 会话在 16:30:07 写入 prepare.ps1，由原 D 盘整合区创建新 D 盘工作区并合并 v2.6.9。 |
| **2026-11-16 19:35 起** | 本轮对话根据用户明确要求统一迁移；当前入口改为 D:/pro/codex-plus-plus，旧 C 盘代码与公共 Git 移入 history。 |

查到的直接证据指向 **Codex 在升级合并时创建 D 盘隔离工作区**，而不是 WorkBuddy 把整个 C 盘仓库迁走。此前 C 盘聊天入口、公共 Git 与 D 盘整合/编译目录同时存在，所以在 C 盘打开聊天仍可能操作 D 盘源码。目录名包含 v266 的旧工作区实际已整合到 2.6.9，不能按名字当成纯 2.6.6。

本次使用 workbuddy-dispatch 技能进行只读任务查询，相关 WorkBuddy 任务记录的 cwd 仍是原 C 盘路径。此查询不能证明它从未触碰 D 盘；归因仅限已经查到的创建动作，没有派发 WorkBuddy 任务。

证据保存于 D:/pro/codex-plus-plus-history/storage-audit-20261116/：codex-creation-evidence.json、codex-v269-creation-evidence.json。另见公共 Git 的两个 worktrees/*/logs/HEAD，以及旧 C 归档中 output/v266-update-20261112-104405/context.json、output/v269-20261114/prepare.ps1。继承进子代理日志的工具记录时间可能是复制时间；实际创建时刻以 Git reflog 和 context.json 为准。

## 完整性复核

13 个工作区的版本、HEAD、分支、MERGE_HEAD 和未提交状态保存在 versions-audit.json。清理后 168 个 Git refs 与清理前一致；git fsck --full --no-dangling 退出码为 0。

以下版本来自工作目录内的 Manager package.json，不代表合并已完成；修改条目数是本次说明文档写入前的快照。所有工作区仍保留。

| 工作区 | 工作版本 | 修改条目数 |
| --- | --- | ---: |
| 当前 codex-plus-plus | 2.6.10 | 24 |
| c-baseline-2.4.0 | 2.4.0 | 10 |
| codexpp-v266-integration-20261112 | **2.6.9** | 291 |
| v267-worktree-20261113 | 2.6.7 | 133 |
| v2.4.0-worktree-20261109-233709 | 2.4.0 | 137 |
| v2.3.1-worktree-20261106-232315 | 2.3.1 | 283 |
| v2.2.0-worktree-20261104-164958 | 2.2.0 | 742 |
| v2.1.4-20261103-000430 | 2.1.4 | 162 |
| v2.1.1-20261029-184313 | 2.1.1 | 272 |
| v2.0.0-20261029-002641 | 2.0.0 | 337 |
| v1.12.1-20261028-114324 | 1.12.1 | 642 |
| subagent-mapping | 1.2.55 | 12 |
| codex-managed-0393 | 1.2.55 | 2 |

当前 bin 与安装目录共 14 项文件的 SHA-256 与最新成功任务一致。Manager 正在 D:/pro/Codex++ 运行，没有重启或替换运行文件。既有产品源码修改保持原状，本次未运行产品测试或 Release；缓存清理记录不充当新源码发布证据。

扫描原始数据为 sizes-201621.json（清理前）、sizes-202311.json（清理后）；空间差额见 cleanup-summary.json。四项 Cargo 预览和执行日志均保存在同一审计目录。
