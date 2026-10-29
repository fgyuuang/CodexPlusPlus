# Windows 日常修改、快速验证与缓存管理

本 fork 基于正式 v2.0.0，使用 PowerShell 7、现有 Rust 工具链、Node 24+ 和已安装的项目依赖。日常修改先跑受影响范围；准备交付时再运行一次[后台发布](release-workflow.md)。纯文档修改无需编译产品。

## 按改动选择最小验证

以下命令从仓库根目录执行，按实际改动选用，不必全部运行。

| 改动 | 开发阶段验证 | 何时需要 Rust 编译 |
| --- | --- | --- |
| 文档 | 检查差异、链接和命令路径 | 不需要 |
| Manager React、样式 | TypeScript、相关前端测试、Vite 热更新 | 正式交付嵌入最新 dist 时 |
| renderer 注入分片 | 重新组装、相关注入测试 | 最终交付时 |
| Rust 核心或数据层 | 对应 crate 的 cargo check、相关测试 | 验证需要运行的代码时 |
| 认证、路由、会话或跨模块迁移 | 定向回归后完整后台发布 | 正式交付必须 |

```powershell
# 只检查所改 crate；比构建所有测试程序和最终链接轻
cargo check -p codex-plus-core --lib
# 数据层改动时换成这个，不必重复跑无关 crate
cargo check -p codex-plus-data --lib

# 以本轮网关谱系为例，过滤到相关单元测试
cargo test -p codex-plus-core --lib gateway::codex_routing -- --test-threads=1
# 只有修改协作接入时才选此集成测试目标
cargo test -p codex-plus-core --test agent_collaboration_integration -- --test-threads=1
```

测试名称过滤只减少执行范围；所选测试程序仍需编译。`--lib` 和 `--test <目标>` 可以避免无关集成测试程序的编译与链接。共享路径集成测试保持单线程。无需反复运行已经通过且不受后续改动影响的检查。

```powershell
# 只做前端类型检查（依赖已安装）
npm --prefix apps/codex-plus-manager run check
# 热更新预览；结束时按 Ctrl+C
npm --prefix apps/codex-plus-manager run vite:dev
```

Vite 页面预览不能证明真实 Tauri 命令、账号路由或桌面行为。需要完整开发窗口时，可在 Manager 目录运行 `npm run dev`，由现有 Tauri 开发命令启动 Vite 和 debug 后端；不要同时另起 Vite 占用 1420 端口。先退出已运行的 Manager，再使用开发窗口，并注意它可能读取同一真实配置，不能当作隔离沙箱。本文只核对了现有配置，未额外启动开发窗口验收。

```powershell
# renderer 分片修改后重新组装
node scripts/assemble-renderer-inject.mjs
```

生产文件由发布脚本交付。不要把 debug EXE 手动复制到安装目录；不要单独 cargo build 后配旧 dist；`npm run build` 是 Tauri 构建入口，日常前端预览使用 `vite:dev`。

## 为什么本次构建慢、缓存大

2026-10-29 成功发布任务 `20261029-123155-f41476b9` 从 12:31 至 13:10，约 38 分钟；其中 Release 编译阶段约 16 分 53 秒，其余主要为完整检查、Rust 测试程序编译和执行。该次已有部分缓存，不能将其作为冷构建或每次修改的固定耗时。

Release 保留 `lto = "fat"`、`codegen-units = 1`、`strip = true` 和 `panic = "abort"`。这些设置面向最终产物；全程序优化与链接耗时较长。日常 `cargo check` 无需执行最终 Release 链接，前端热更新也无需每次重编 Rust。

debug 同时容纳依赖、符号、增量缓存和多个测试程序；独立 worktree 默认还会各自建立 target。不要为每个小任务重复冷构建，也不要每次编译前清缓存。不同候选版本不要并行共用可变 target；合并试验目录的清理需另行核对。

## 可选：减少开发符号体积

需要持续节省空间时，可在专用开发 PowerShell 窗口先设置：

```powershell
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
# 然后运行上方所需的 cargo check / cargo test
```

这只减少开发和测试的调试符号，不改变 Release 优化。代价是原生调试、崩溃定位可用的信息减少；切换设置会使相关缓存失效，首次仍需重建。统一保留同一组设置才有复用收益；遇到需调试崩溃时恢复符号。增量编译继续使用 Cargo 默认值，不为省空间默认关闭。本文没有修改 Cargo.toml、全局环境或发布脚本，也未实测这组设置的速度或空间收益。

关闭该开发窗口即可撤销临时变量。启动正式发布前使用普通窗口，避免临时设置被后台进程继承。不要在开发窗口内调整 Release 环境变量来绕过正式优化要求。

## 缓存回收与交付

先确认无 Cargo、Rustc、Link 或 Tauri 编译任务，核对当前仓库和 target 路径，再预览、执行：

```powershell
cargo clean --profile dev --dry-run
cargo clean --profile dev
```

本轮删除 debug 文件 56,492 个，逻辑体积 88.3 GiB，C 盘实际回收 81.03 GiB；Release 缓存约 4.02 GiB 保留。不同文件系统及压缩会使逻辑体积与实际释放空间不同。清理后下一次开发检查需要重建，不能同时期待冷缓存和最快增量编译。

保留 bin、Manager dist、原生运行库缓存、发布日志、原有配置、安装及合并备份。node_modules 是已安装依赖，不作为本次垃圾清除；删除后还需重新安装。对备份和旧试验目录，先列清单、区分在用与待审查内容，再另行决定，不能按名称批量删除。

最终交付仍运行 `scripts/release.ps1 -Action Start`，以 `state=succeeded`、完整测试、源码冻结和产物哈希为准；任务运行时停止开发修改，不重复启动。修改频繁时先积累一组已定向验证的改动，再统一发布。仅调整文档后不必重新跑 38 分钟流水线。
