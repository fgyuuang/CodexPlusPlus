# Windows 日常修改、快速验证与缓存管理

本 fork 当前源码整合目标为正式 v2.6.6，是否已交付以成功的 release job 为准；本轮隔离源码位于 `D:/pro/codexpp-v266-integration-20261112`。使用 PowerShell 7、现有 Rust 工具链、Node 24+ 和已安装的项目依赖。日常修改先跑受影响范围；准备交付时再运行一次[后台发布](release-workflow.md)。纯文档修改无需编译产品。

开发阶段只做必要的定向验证，完整测试统一留给一次后台发布任务。流程固定为「全部检查及测试通过 → 前端构建 → 正式 Release 编译 → 核对交付」；全通过后自动前进，不需要再次放行，也不在正式编译阶段重复测试。`cargo test` 编译的是测试程序，应在进度中明确说明。发布期间保留缓存并冻结输入，不因耗时重新启动任务。详细阶段与监视规范见[测试与编译的固定交接](release-workflow.md#测试与编译的固定交接)。

发布启动后继续跟踪至交付核对完成，不能以“已提交后台任务”结束。检查和测试期间及时检查失败；正式优化、链接阶段每次等待 45–60 秒后核对状态和进程活动，正常耗时不重启、不清缓存。失败时记录步骤、退出码和日志，确认旧编译进程退出后修复，按新输入启动唯一任务并继续跟踪。只有任务 succeeded 且产物、备份及目标哈希核对通过，才报告构建交付完成；用户重启和界面验收单独说明。

## 按改动选择最小验证

以下命令从仓库根目录执行，按实际改动选用，不必全部运行。

| 改动 | 开发阶段验证 | 何时需要 Rust 编译 |
| --- | --- | --- |
| 文档 | 检查差异、链接和命令路径 | 不需要 |
| 独立用户拓展的 JS/Node 后台、页面 | Node 语法/合约测试，经拓展工具写入、核对加载与界面 | 既有通用 API 足够时不需要 |
| Manager React、样式 | TypeScript、相关前端测试、Vite 热更新 | 正式交付嵌入最新 dist 时 |
| renderer 注入分片 | 重新组装、相关注入测试 | 最终交付时 |
| Rust 核心或数据层 | 对应 crate 的 cargo check、相关测试 | 验证需要运行的代码时 |
| 认证、路由、会话或跨模块迁移 | 定向回归后完整后台发布 | 正式交付必须 |

```powershell
# 只检查所改 crate；比构建所有测试程序和最终链接轻
cargo check -p codex-plus-core --lib
# 数据层改动时换成这个，不必重复跑无关 crate
cargo check -p codex-plus-data --lib

# 网关选路采用上游，验证 helper 到模拟上游的真实请求链
cargo test -p codex-plus-core --test gateway_e2e -- --test-threads=1
# 只有修改协作接入时才选此集成测试目标
cargo test -p codex-plus-core --test all agent_collaboration_integration:: -- --test-threads=1
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

## 独立拓展的快速更新

只涉及独立服务管理、页面或已有拓展接口时，先读取关联拓展并保留原功能，在仓库外源码目录做修改和 Node 定向测试，再通过 `codexpp_extension_write` 更新同一个用户包。检查返回的加载状态与错误，页面变化再核对截图；无需运行 Cargo、Vite 或替换 Manager。若旧脚本阻止安全热重载，保留已写入文件，等待用户重启或手动热重载，不能把 `not_loaded` 报成成功加载。

本轮「本地服务中心（独立）」源码与测试在 `D:/pro/codexpp-local-services-extension-20261112`，原供应商、账号库和服务数据不随拓展更新迁移。真实进程启停仍需独立验收，mock 与只读健康检查不能代表真实操作已通过。账号、协议代理或历史兼容层尚未全部迁出；修改这些 Rust 能力仍需正式发布，不能直接用纯上游 EXE 覆盖并声称全部保留。

## 为什么本次构建慢、缓存大

2026-10-29 成功发布任务 `20261029-123155-f41476b9` 从 12:31 至 13:10，约 38 分钟；其中 Release 编译阶段约 16 分 53 秒，其余主要为完整检查、Rust 测试程序编译和执行。该次已有部分缓存，不能将其作为冷构建或每次修改的固定耗时。

Release 保留 `lto = "fat"`、`codegen-units = 1`、`strip = true` 和 `panic = "abort"`。这些设置面向最终产物；全程序优化与链接耗时较长。日常 `cargo check` 无需执行最终 Release 链接，前端热更新也无需每次重编 Rust。

debug 同时容纳依赖、符号、增量缓存和多个测试程序；独立 worktree 默认还会各自建立 target。不要为每个小任务重复冷构建，也不要每次编译前清缓存。不同候选版本不要并行共用可变 target；合并试验目录的清理需另行核对。

## 可选：减少开发符号体积

本轮后续同步已吸纳上游 `5e1d8c1e`：Cargo.toml 默认让开发/测试构建仅保留行号表，依赖不带调试符号；Release 的 fat LTO 等设置保持原样。优先复用这一默认配置，不需要每个终端再设置变量。本轮已同步上游测试程序合并，主集成目标为 `--test all`；独立进程边界用例保留原测试目标。

需要持续节省空间时，可在专用开发 PowerShell 窗口先设置：

```powershell
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
# 然后运行上方所需的 cargo check / cargo test
```

这会进一步去掉开发和测试的行号信息，不改变 Release 优化。代价是原生调试、崩溃定位可用的信息减少；切换设置会使相关缓存失效，首次仍需重建。统一保留同一组设置才有复用收益；遇到需单步查看变量时可在专用开发窗口设置 `CARGO_PROFILE_DEV_DEBUG=2` 和 `CARGO_PROFILE_TEST_DEBUG=2`。增量编译继续使用 Cargo 默认值，不为省空间默认关闭。没有修改全局环境或发布脚本，也不把上游 A/B 数据当作本机提速结果。

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
