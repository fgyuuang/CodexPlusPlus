# 官方体验次账号模型显示修复

## 问题与证据

使用账号一时缺少带账号名称的 Sol，使用账号二时正常。检查发现两个账号官方目录相同，均有 7 个可显示模型，但账号二的元数据状态为 `needsReauth`。次账号路由只接纳启用且 `ready` 的账号；当前账号的原生模型则直接来自其目录。因此产生方向不对称。

官方目录已多次刷新成功，但成功路径未修复认证状态。此前为避免旧 live 凭据覆盖新登录，合并同一或较旧凭据时有意保留 `needsReauth`；这条保护正确，缺少的是独立的真实官方认证成功确认路径。

另一个可复现问题是：次账号目录还会与活动账号的可信模型集合取交集。两个账号模型权限不同时，会隐藏仅次账号可用的模型。回归测试用主账号 Sol、次账号 Luna 的不同目录复现并约束此行为。

## 变更范围与约束

按 `code-quality-workflow` 执行两项局部修复，合计三个 Rust 源文件及状态机文档，源代码和测试增量为 +125/-3 行；无结构重写、依赖或持久化格式变更。变更前原始文件保存在 `output/official-source-menu-20261009/`。

- `official_accounts.rs`：只有用量请求真正成功时确认认证；在账号库锁内核对请求使用的完整凭据和请求前账号快照，满足条件才恢复旧 `needsReauth`。并发凭据、启用状态或元数据变化则保留最新状态。
- `official_model_catalog.rs`：模型目录真实 200/304 成功也执行相同确认；网络错误、目录回退或本地缓存不会确认认证。
- `official_experience.rs`：次账号模型使用该账号缓存目录，不再与主账号取交集；仍过滤隐藏、不支持 API、禁用或未就绪账号，保留稳定账号 provider、准确模型别名及严格路由。
- 不恢复过时 live 凭据，不强制刷新令牌，不改账号选择、默认 provider 或历史会话索引。

## 已完成验证

- 修改前直接官方路由基线 2 项通过。
- 官方相关测试 49 项通过。测试覆盖不同账号目录、账号切换、禁用/失效过滤、历史路由、陈旧认证状态恢复、并发账号更新和凭据替换。
- 核心库及集成测试共 1,554 项通过、7 项忽略，包括 599 项库测试；排除的已知基线失败见下文。
- 数据层 195 项通过、1 项忽略；Manager 101 项通过。
- `cargo fmt --all -- --check`、工作区 `cargo check -j 2 --workspace` 和本次文件的 `git diff --check` 通过；现有编译警告未改动。
- TypeScript 检查通过；前端测试 379 项通过、1 项跳过。
- 编译本次核心库的诊断程序，用当前账号的现有令牌请求官方用量接口成功，账号二由 `needsReauth` 恢复为 `ready`。内存中完整凭据前后相等，live `auth.json` SHA-256 不变。
- 诊断程序只在内存中切换主账号设置：分别以账号一和账号二生成目录，均有 7 个次账号路由、3 个 Sol 路由，且所有这些路由都存在于生成的 catalog。未实际切换当前登录或停止聊天。
- 通过运行中桌面端的 CDP 和既有 bridge 读取 `/codex-model-catalog`：状态 `ok`，31 个总路由、7 个次账号路由、3 个带账号名称的 Sol 路由。
- `config.toml` 只读核对：默认 provider 仍为 `openai`，两个官方账号 provider 定义仍在。本次未编辑该文件。

默认并行执行全量核心测试时，Windows 内存耗尽，链接器报 LNK1171 / 错误 1455。已核对并结束仅本次测试的进程树，保留 Codex/Manager，改用 `cargo -j 2`。最初失败日志保留供审查。

全量核心测试明确排除已知基线失败 `cliproxy_general_aliases_keep_non_official_models_and_yield_to_special_official_channel`，不能据此声称未排除的完整套件全部通过。

## 发布与实际验收

`cargo build -j 2 --workspace --release` 成功完成，耗时 13 分 12 秒。确认所有 cargo/rustc/link/tauri 编译进程结束，源码哈希仍与验证版本一致后，备份并更新 `bin` 与实际运行目录 `D:\pro\Codex++`。Manager 和启动器的时间戳与 SHA-256 均已更新，Manager 时间晚于现有前端 `dist`；前端源文件未改动，未重复生成前端。

以下 SHA-256 在 `target/release`、`bin`、`D:\pro\Codex++` 三处逐项一致：

| 程序 | SHA-256 |
|---|---|
| Manager | `286A0984FF2DDF5E48166DA917E064B7B5E2D9279AE51CABFE5AEA59793C903D` |
| 启动器 | `4EE150082924A450EEDAD13B5422AD3E086E5EBE77BC560D2969B8F864EA239B` |
| Mobile relay（未改变） | `0C22645ADF37A4C65E4462D196A95FEBF5F0347859A391B8A00B75AEEBA3B718` |

旧运行程序保留在 `D:\pro\Codex++\backups\official-source-menu-20261009\`，部署清单位于证据目录的 `deployment.json`。本次没有停止 Codex/Manager，没有提交或推送。

部署后的 Manager PID 47632 仍创建于 10 月 8 日 20:49，启动器 PID 49696 创建于 10 月 9 日 10:58，均早于本次编译。磁盘上的新文件不会替换已加载的进程代码。已遵守用户先前选择稍后手动从托盘退出重启的安排；需完整退出 Codex 与托盘 Manager，从 `D:\pro\Codex++\codex-plus-plus-manager.exe` 启动新版，再切回账号一验证实际模型列表。账号状态恢复、当前桌面 bridge 和两种配置的生成目录已验证，最终切换账号一后的新版 GUI 验收仍待这次手动重启。

全仓 scope 检查会包含此前未提交的上游合并、大量本地修改及未跟踪证据/二进制，超过技能默认预算；完整结果保留在 `full-scope-check.log`。本次以逐文件变更前副本与最终源文件对比复核实际增量，不宣称全仓预算检查通过。未提交或推送。
