# 官方账号原生模型请求兼容修复

## 原因与范围

用户报告账号专用 provider 收到 `gpt-6.1-sol` 时出现“模型不属于来源 provider，已关闭式拒绝”。两个账号都启用且 `ready`，运行路径为 `D:\pro\Codex++`，文件 SHA-256 与 10 月 9 日修复产物一致。此次问题属于模型表示兼容。

菜单使用 `<账号 provider>:<官方模型>` 路由别名；实际 Codex 请求和已有会话也会使用“账号专用 provider + 原生裸模型名”。只读 SQLite 检查确认这两种保存格式均存在。旧代理解析器只匹配前一种格式，合法请求在读取账号凭据前就被拒绝。

按 `code-quality-workflow` 执行局部修复：只改 `official_experience.rs` 的账号来源模型解析及既有回归测试，同步状态机文档；源码增量 +54/-6 行，无依赖、格式或接口重写。

## 行为约束

- 账号专用 provider 可以使用原生模型名或该账号的完整路由别名，统一解析为保留原账号 ID 的 `DirectOfficial` 描述。
- 裸模型只能由该账号自己的缓存目录证明；缺少缓存立即拒绝，不借用主账号目录。
- 未知账号、未知模型、跨账号别名、禁用和未就绪账号继续拒绝。
- 请求仍由既有官方路径使用指定账号的存储凭据直连 ChatGPT，保持 `strict`；不使用当前 live 登录替代目标账号，不进入供应商轮转。
- 旧别名的兼容目录回退也明确设置为目标账号。菜单显示、默认 provider、全局配置、登录凭据与会话 SQLite/rollout 本次均未修改。

## 验证

变更前基线 2 项通过。扩展原有测试后，原生模型断言在旧实现中返回 `None` 并失败；修复后 2 项聚焦回归通过。测试覆盖账号成为主账号前后、不同账号目录、目录缺失、未知模型、未知 provider、跨账号别名、需要登录和禁用状态。

使用本次核心库编译的只读诊断程序，对两个真实账号分别调用生产解析器：`gpt-6.1-sol` 和账号完整别名都解析成功，源账号 ID 精确一致、上游模型保持原值、失败策略为 `strict`；未知模型和另一个账号别名均被拒绝。没有向官方发送聊天请求，也没有修改会话或凭据。

TypeScript 检查通过；前端测试 379 项通过、1 项跳过。全量核心回归继续明确排除已知基线失败 `cliproxy_general_aliases_keep_non_official_models_and_yield_to_special_official_channel`；不能据此声称无排除的完整套件全绿。

核心及集成测试 1,554 项通过、7 项忽略；数据层 195 项通过、1 项忽略；Manager 101 项通过。工作区 `cargo check -j 2 --workspace`、`cargo fmt --all -- --check` 及本次文件的 `git diff --check` 通过。

证据、变更前源文件及日志位于 `output/official-wire-model-20261011/`，包括 `reproduction-final.log`、`focused-tests.log`、`probe-result.log`。

## 发布与实际验收

`cargo build -j 2 --workspace --release` 成功完成，耗时 14 分 49 秒。确认编译进程全部退出、源码哈希与验证版本一致后完成部署。Manager 和启动器的时间戳及 SHA-256 相比旧产物均更新；Manager 时间晚于已有 `dist`。前端与 renderer 分片没有修改，未重复生成前端。

以下 SHA-256 已验证在 `target/release`、`bin`、`D:\pro\Codex++` 三处逐项一致：

| 程序 | SHA-256 |
|---|---|
| Manager | `931E29C7DD5BD190BAB68350356CB2860B5CDC3D32B26164BB92F34D74F3070C` |
| 启动器/协议代理 | `9F1A12BDBD5E32FB59A5B89495C8B677379BBB6AE273C3914484828B6E472445` |
| Mobile relay（未改变） | `0C22645ADF37A4C65E4462D196A95FEBF5F0347859A391B8A00B75AEEBA3B718` |

旧程序保留在 `D:\pro\Codex++\backups\official-wire-model-20261011\`，部署清单和进程记录在证据目录。未提交或推送。部署后只读配置检查通过：默认 provider 为 `openai`，两个账号 provider 定义仍在。

全仓 scope 检查包含此前未提交的合并、本地扩展及未跟踪证据/二进制，退出码 2，超出技能默认预算；结果保存在 `full-scope-check.log`。本次用变更前源文件副本逐项复核源码增量，不宣称全仓预算检查通过。

保留用户先前选择手动退出重启的安排，未停止聊天或 Manager。部署后 Manager PID 42148 创建于 10 月 9 日 19:55，启动器 PID 43232 创建于 10 月 11 日 18:50，早于本次编译，仍执行旧代码。请完整退出 Codex 与托盘 Manager，从 `D:\pro\Codex++\codex-plus-plus-manager.exe` 启动新版，再继续报错会话。源码/真实目录的解析预检已完成，新进程下的实际聊天验收仍待这次手动重启。
