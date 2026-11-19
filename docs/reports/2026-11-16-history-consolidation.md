# 历史收敛与当前主仓库（2026-11-16）

用户要求仅保留当前开发基线，将需要保留的历史材料统一放到 D:/pro/codex-plus-plus/history，久远版本不再维持完整开发工作区。

后续目录归拢（同日）：用户进一步要求将 D:/pro 下剩余四项也移入 history，已完成。两个插件源码目录位于 history/plugins；旧 v269 Junction 位于 history/consolidation-20261116/retired-junctions/old-v269-entry，旧历史空壳位于 history/consolidation-20261116/empty-old-history。D:/pro 下 Codex++ 相关主入口现仅有 codex-plus-plus 和 Codex++。下表容量为先前阶段快照，此次只移动约 45 MiB 的插件材料，不释放磁盘空间。

两个插件共 3238 个文件在移动后、修改目录说明前逐个 SHA-256 一致，独立插件的 .git 也一并保留。随后仅修正 preview.mjs 与 build-ui.mjs 的宿主路径及两份 README，两处脚本 node --check 通过，所指宿主桥接及 Lucide 文件存在。主仓库 HEAD 和 index 哈希不变，git worktree list 仍只有当前主工作区；history 仍被 Git 排除。未编译产品、重新安装插件、修改外部服务或推送。原发布封存中的旧绝对路径保留，不以此次归拢宣称旧任务 Redeploy 已验证；查阅历史记录时映射到当前主目录。操作、文件校验与后续路径修订证据见 history/consolidation-20261116/plugin-relocation-*.json。

## 当前目录

| 位置 | 用途 | 逻辑大小 |
| --- | --- | ---: |
| D:/pro/codex-plus-plus（不含 history） | 当前 2.6.10 源码、依赖、最新发布证据与唯一主 Git | 3.44 GiB |
| 上述目录中的 .git | 完整提交历史、分支、索引与对象库，包含在上一行中 | 2.02 GiB |
| D:/pro/codex-plus-plus/history | 统一收纳的历史材料，未彻底删除 | 26.36 GiB |
| D:/pro/Codex++（跳过备份目录链接） | 安装目录其余文件 | 0.59 GiB |

当前项目目录总计约 29.80 GiB，其中 26.36 GiB 为 history。跨项目与安装目录合计约 30.39 GiB，不重复统计 Junction。旧 D:/pro/codex-plus-plus-history 没有文件，仅剩空目录；删除空目录也被审批拦截，没有用其他机制重试。

当前 .git 已从旧 2.4.0 目录移入当前仓库，并用原活动工作区的 HEAD、index、ORIG_HEAD、HEAD reflog 等恢复其原开发状态。现在 git worktree list 只登记 D:/pro/codex-plus-plus 一个工作区，分支仍是 feat/reset-cards-2.6.10，HEAD 仍为 b8dd3208bad40869d55244e4af6fa6c9963f7cbd。主仓库不再依赖 history 中的任何旧 Git 副本。

## “迁移副本”具体是什么

它们是搬盘时为核对和回滚额外复制的材料，并非当前开发所需的另一套基线：

- history/legacy/migration-20261116-193527/verified-copies：约 10.76 GiB，原 C 盘仓库及其历史工作区的验证副本。
- 同目录的 git-before：约 2.02 GiB，迁移前公共 Git 的整份副本。
- history/legacy/c-baseline-2.4.0：已经退役的旧 C 盘目录及其发布输出；现在不再持有公共 Git。
- history/installed-backups：约 5.56 GiB，历次安装交付的备份。原 D:/pro/Codex++/backups 保留为指向这里的 Junction，以保持发布备份的已有路径可用，没有复制第二套数据。
- history/legacy/storage-audit-20261116 和原迁移目录内的小型 JSON/日志：用于记录本次迁移和清理结果。

以上数据都已集中到用户指定的 history 中。归拢和路径链接本身不释放空间。

## 旧开发工作区已退役

12 个旧工作区的独有未提交内容已提取成恢复压缩包，总计 82,169,342 字节（约 78.36 MiB），放在 history/consolidation-20261116/recovery。每个包包含原 HEAD、状态、Git 索引与合并元数据、暂存和未暂存的二进制补丁、独有未跟踪文件，以及另存的旧私有配置。编译 EXE/DLL/PDB、完整未修改源码、node_modules 和旧 output 编译结果不作为恢复内容。

12 个压缩包均重新执行 ZIP CRC 检查，并保存 SHA-256。对应基准提交通过原有分支和 11 个 refs/archive/20261116/* 引用保护，避免解除旧工作区登记后基准提交被垃圾回收；没有重写、删除或推送原分支。

原主工作区在主 Git 提升后退役；其余 11 个 linked worktree 已退出登记。除首次 1.12.1 清理外，后续清理均使用 git -c core.longpaths=true worktree remove --force：首次 1.12.1 删除因 Windows 长路径失败，Git 已撤销其登记但仍有文件残余，残余位于 history/legacy 中，未冒充完整删除。其他 10 个 linked worktree 清理成功。

清理前已将旧工作区中的 9 个目录链接单独移入 retired-junctions，避免清理时穿透共享依赖或测试链接的目标。该目录中的链接不是另一套源码，不应跟随统计。

本轮 D 盘实际可用空间从约 187.94 GiB 增至约 191.43 GiB，新增回收约 3.49 GiB。上一阶段已完成的编译缓存清理另计约 83.96 GiB，不重复计入本轮。最终数值以前后磁盘快照为准。

## 尚未执行的删除

自动审批以 blocked by policy 拒绝了对 verified-copies 和 git-before 的删除预览；连空目录移除也被拒绝，没有给出更具体原因。没有换工具重试这些被拒绝的动作，迁移重复副本仍保留。此前被拒绝的旧发布二进制清理也没有执行。

因此已实现“只有一个当前开发工作区，历史统一收到 history”的方案，但没有实现“磁盘上只剩核心内容”。history 的 26.36 GiB 不能算作已清理空间。后续清理须依据本轮明确的目录和依赖边界，不能再以“公共 Git 仍在旧目录”为保留理由，也不能将审批拒绝说成用户没有授权。

## 核对与证据

证据目录：D:/pro/codex-plus-plus/history/consolidation-20261116。

- final-integrity.json：原 168 个 refs 全部原值保留；新增 11 个恢复 refs；活动 index 的 SHA-256 与提升前一致；HEAD 和分支保持原值；仅登记当前工作区；12 个恢复压缩包 CRC 与哈希。
- git-fsck-after-promotion.json、git-fsck-final.json：主 Git 迁移和旧工作区退役后的 git fsck --full --no-dangling 均退出 0。
- recovery-archives.json、worktree-removals.json、oldest-worktree-removal.json：每个归档与清理结果，包括长路径失败的残余。
- child-moves.json：历史父目录整体移动失败后，分别归拢的结果；旧 C 盘目录最终由 48 个顶层条目迁入 history，原路径不再包含文件。
- installation-backups-moved.json：安装备份收纳位置及兼容链接。
- final-storage.json：跳过所有重解析点后的目录统计，扫描无错误。

当前源码的原有未提交状态在 Git 提升后核对一致，之后仅更新本轮路径说明与 AGENTS.md。当前 bin 和运行安装的 14 项产物哈希与原成功任务一致；没有替换或重启运行版本，没有合并、提交、推送或产品编译。

history 通过 .git/info/exclude 的 /history/ 排除，其中可能包含私有配置，不提交或推送，也不再作为开发或升级基线。保留旧版本的 Git 提交不等于保留多套完整源码目录。
