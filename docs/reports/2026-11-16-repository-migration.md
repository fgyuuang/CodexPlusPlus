# Codex++ 仓库迁移与历史备份清单

日期：2026-11-16（北京时间）。本轮迁移源码、Git、未提交修改、历史工作区与原有发布材料，并按清理预览移除 C 盘编译缓存；没有合并、提交、推送、重装依赖或重新编译产品。

## 后续开发入口

主开发目录：`D:/pro/codex-plus-plus`。

当前分支：`feat/reset-cards-2.6.10`；HEAD：`b8dd3208bad40869d55244e4af6fa6c9963f7cbd`。迁移前的 24 条文件状态记录原样保留，另新增本次迁移说明并更新 AGENTS.md 的版本与路径指引。AGENTS.md 修改前备份在迁移证据目录 `active-AGENTS-before.md`。

运行软件仍在 `D:/pro/Codex++`。安装目录、开发源码、历史工作区是不同用途，不应互相覆盖。当前成功交付任务为 `20261116-165859-52005205`；迁移后 1474 个冻结构建输入哈希仍一致，已安装 Manager 的哈希也与任务一致。

原 C 盘主目录已不再包含代码、Git 或编译缓存。它仅因 PSForge 工具进程占用日志而保留，另有一份 AGENTS.md 迁移指引；在 Codex 中打开 D 盘主开发目录继续工作。

## 历史与 Git 位置

| 目录 | 内容与用途 |
| --- | --- |
| `D:/pro/codex-plus-plus-history/c-baseline-2.4.0` | 原 C 盘主工作区，旧 `codex/Manage-multiple-official-accounts` 分支及其未提交修改；包含原有 bin、output 与贡献准备材料 |
| 上述目录的 `.git` | 仍在使用的公共 Git 数据库，保存所有分支、远端、对象、reflog、索引与 worktree 注册；不能因目录名为 history 就删除 |
| `.../worktrees/integration` | v1.12.1、v2.0.0、v2.1.1、v2.1.4 等历史整合工作区 |
| `.../worktrees/subagent-mapping` | 原子代理模型映射工作区；C 盘旧 dev 缓存已清理 |
| `.../worktrees/codex-managed-0393` | 原 Codex 管理的 detached 工作区；旧 C 盘入口保留目录链接 |
| `.../worktrees/codexpp-v266-integration-20261112` | 原 D 盘中期整合工作区，保留其后续演进与未提交修改，不能仅按目录名判断实际版本 |
| `c-baseline-2.4.0/output/*worktree*` | v2.2.0、v2.3.1、v2.4.0、v2.6.7 等嵌套历史工作区，随原主目录一并迁移 |

共 13 个工作区均已注册到 D 盘实际路径，HEAD、分支和未提交状态与迁移前一致；168 个 Git 引用与远端配置未改变。`git fsck --full --no-dangling` 退出码为 0。

旧工作区中存在较多未提交修改，因此保留了完整目录，未用 tag checkout 或仅导出 commit 的方式代替。没有对它们做 reset、stash、自动提交或去重删除。

## 哪些是真正的备份

本次证据目录：`D:/pro/codex-plus-plus-history/migration-20261116-193527`。

| 材料 | 用途 |
| --- | --- |
| `git-before` | 迁移前完整 Git 数据库备份；5813 个文件已逐个核对哈希 |
| `verified-copies/c-source-1` | 原 C 盘主工作区的已验证复制备份，包含原 Git 和历史输出；不是新的开发目录 |
| `verified-copies/c-source-2` | 原 C 盘 integration 集合的已验证复制备份 |
| `verified-copies/c-source-3` | 原 C 盘模型映射工作区的已验证复制备份 |
| `verified-copies/c-source-4` | 原 Codex 管理工作区的已验证复制备份 |
| `original-junctions` | 改写路径前保留下来的 10 个目录链接；不包含另一份依赖或源码数据 |
| 各工作区 `output/release-jobs` | 原有发布日志、输入封存、产物哈希、交付备份和故障证据，全部保留 |
| 原有 backup、.bak、回滚材料 | 分别保留在原相对目录；未因旧版本或命名重复而自动清理 |

`verified-copies` 是本次迁移回滚材料，会占用 D 盘额外空间；其中旧 Git 路径需按清单修复后才能恢复使用，不在这里日常开发。当前 history 目录整体并非可丢弃的备份，它包含正在使用的公共 Git。

原有输出目录分类详见 `output-classification.json`，发布任务详见 `release-jobs-before.json`。遗留的 6 个 `running` 记录均未发现 PID 与启动时间匹配的原进程，结果在 `stale-job-audit.json`；原始记录未改写，不将其当作正在正常构建。

此前贡献准备报告位于历史主工作区的 `docs/reports/2026-11-16-upstream-contribution-preparation.md`，原始重置卡补丁及 PR 草稿在其 `output/contribution-prep/20261116-190539`；没有在迁移时丢弃或自动提交。

## 空间清理与校验

C 盘三个清理范围分别为主工作区 dev、主工作区 release、旧映射工作区 dev。先执行 Cargo dry-run，再执行相应 profile 清理；Cargo 报告逻辑文件大小分别为 18.1、7.0、5.6 GiB，共约 30.7 GiB。原生库下载缓存与正式交付材料保留并迁到 D 盘。

D 盘现有 target 缓存保留，未顺带执行全盘清理。它们是编译缓存，不是备份；完整缓存与源文件分别统计，避免误删仍有用的数据。

迁移后对六组目录中原有的 79705 个非 Git、非 target 文件逐个进行 SHA-256 核对，全部一致。Git 另以完整数据库备份、引用对比和 fsck 核实；已清理的 C 盘 target 不冒充仍有完整备份。D 盘工作区为同盘目录移动，原编译缓存保留。

只更新了路径说明文档，没有产品行为变更，因此未重新启动测试或 Release。原成功发布证据与本次迁移证据分别保存。

## 剩余日志与兼容入口

`C:/Users/fgy/Documents/codex-plus-plus/psforge_debug.log` 为 2448399 字节，原件被 PSForge 工具进程占用，复制到 D 盘后无法移除，目录也无法改名。未强制关闭这些工具；C 盘留下的是日志与迁移指引，不是第二套源码。工具退出后方可处理最后这份原日志。

旧 D 盘开发入口以及其他 C 盘工作区入口保留了目录链接；实际数据在 D 盘。开发必须使用新的主入口，避免在原 C 盘目录重新生成仓库或缓存。

迁移中一次批量移除旧链接的操作被自动审批以 `blocked by policy` 拒绝，未执行。随后使用保留已验证副本的目录迁移，并将旧链接改为在 D 盘留档；没有重试同一批量删除操作。

详细验收记录见证据目录中的 `files-verified.json`、`git-verified.json`、`git-fsck-result.json`、`release-inputs-after.json` 和 `final-summary.json`。最终磁盘空间以前后快照为准。

## 后续空间清理（2026-11-16）

再后续：当前仓库已提升为独立主工作区，公共 Git 位于自身 .git，旧目录已不被依赖。历史集中到 history；最新状态见 [历史收敛报告](2026-11-16-history-consolidation.md)。本文保留搬盘时的阶段记录，不再以旧公共 Git 路径指导操作。

用户追加要求后，已清理 D 盘当前开发区与历史整合区的 dev/release 编译缓存。相关目录逻辑大小从约 125.35 GiB 降至 33.87 GiB，D 盘实际可用空间增加约 83.96 GiB。旧二进制批量删除被自动审批拒绝，迁移副本去重仅审计，二者未执行删除。迁移时的“保留 D 盘 target”描述是当时状态，最新明细以 [空间审计](2026-11-16-storage-audit.md) 及 history/storage-audit-20261116 原始证据为准。
