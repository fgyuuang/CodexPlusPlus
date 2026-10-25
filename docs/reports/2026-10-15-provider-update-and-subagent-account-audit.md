# 2026-10-15：供应商更新与 Ultra 子代理账号审计

## 结论及证据边界

用户指定聊天：`01a13cf5-985a-76e1-99c5-201d088f9f9e`。

确认发生过父聊天切换来源后，旧子代理仍通过自己的 `openai` 来源执行后续任务。不能据此声称所有新子代理都会越过账号路由。另确认 renderer 存在独立、可复现的缺陷：账号专用 provider 搭配官方裸模型时被重写为 `openai`；本轮修复这个缺陷，并保存恢复回合所需的 model/provider 对。

账号专用 provider 代表固定本地账号 ID；`openai` 代表桌面端原生登录链路，不是固定账号 ID。它使用的认证受桌面端全局登录状态影响。审计开始时 live 登录匹配本地存档 `8a190452…`，稍后再次收集时已匹配 `b49e1c91…`；本轮未写入真实凭据或配置，不能把最后一次身份快照倒推给所有历史请求。

现有日志缺少逐请求 thread 关联，且会轮换，因此无法追溯出完整的官方账单或把所有额度下降精确分摊给每个子代理。可以确认来源差异和持续发生的 token/额度事件；本轮为后续代理请求补充安全关联日志。

| 优先级 | 发现 | 处置 |
|---|---|---|
| P1 | 显式账号 provider 搭配官方裸模型被 renderer 重写为 openai | 已修复并回归验证 |
| P1 | 切父聊天来源后，旧子代理仍可在自己的来源继续产生用量 | 历史记录确认；新增提示，既有子代理需停止后重建，尚未自动迁移或阻断 |
| P2 | 新版 catalog 要求 base_instructions 为字符串 | 已修复缺失/非法字段兼容 |
| P2 | 旧出口日志缺少 thread 关联，无法完整分摊历史账单 | 已加后续请求关联日志；历史归因限制保留 |
## 指定聊天时间线（北京时间）

| 时间 | 父聊天 / 子代理状态 | 判断 |
|---|---|---|
| 10 月 15 日 08:30:59 | 父回合 `gpt-6.1-sol / Ultra`，当时原生来源为 `openai` | 不应以父聊天后来的 provider 归因这轮 |
| 08:31:30～08:31:46 | 创建 `latency_trace_audit`、`preset_flow_cost`、`draft_visibility_audit` 三个子代理，都是 `openai / gpt-6.1-sol / Ultra` | 创建时继承一致 |
| 11:35:13 | 父聊天改用 `codex_plus_official_b49e1c91…:gpt-6.1-sol` | 父聊天已切到专用账号来源 |
| 11:43:40 | 父聊天改用同账号的 `gpt-6-luna / Ultra` | 账号未变，只切模型 |
| 11:35～12:18 | 前述三个旧子代理仍以 `openai` 执行多次后续回合，并产生额度更新 | 父聊天切换没有迁移存活子线程，存在多来源并行消耗 |
| 11:52:54 | 新建 `settings_ui_audit` 子代理，provider 和模型均继承 `b49e1c91…` 的 Luna 来源 | 新建子代理没有在这条记录中切回 `openai` |

另核对 10 月 12 日 06:49、07:02、07:42 三个 Luna Ultra 子代理：均保留父回合的 `codex_plus_official_8a190452…` provider 和模型别名。这组记录不支持“Ultra 新建子代理必定走桌面登录”的判断。

安全证据保存于 `output/provider-audit-20261015/routing-evidence.json`。收集脚本只输出白名单中的模型、provider、父子关系、时间和额度计数，不输出聊天正文、提示词或令牌。rollout 中包含继承历史，分析只采用子代理创建之后的自身回合，避免把继承片段当作真实新请求。

## 与本机版本一致的官方实现核对

本机 Codex CLI 为 `0.162.0-alpha.17.2`。对应 tag 的 `agent/child_config.rs` 从父回合继承 provider；显式 model override 调整模型和推理等级，不自动改成 `openai`。`agent/control.rs` 的继续任务逻辑向已有子线程发送输入，已加载子线程的 resume 直接复用线程，不会重新以父聊天当前 provider 创建。

- [创建时继承 provider 的实现](https://github.com/openai/codex/blob/rust-v0.162.0-alpha.17.2/codex-rs/core/src/agent/child_config.rs)
- [已有子代理继续执行的实现](https://github.com/openai/codex/blob/rust-v0.162.0-alpha.17.2/codex-rs/core/src/agent/control.rs)
- [已加载线程的 resume 实现](https://github.com/openai/codex/blob/rust-v0.162.0-alpha.17.2/codex-rs/core/src/agent/control/resume.rs)
- [官方子代理说明](https://learn.chatgpt.com/docs/agent-configuration/subagents)

## 上游最新版本与本地差异

最新发布为 [v1.9.0](https://github.com/BigPizzaV3/CodexPlusPlus/releases/tag/v1.9.0)，北京时间 10 月 15 日 01:39，tag 指向 `a5f531d0d2d510a984890f26fdbbc98d995c12ad`。已只读 fetch，`origin/main=e2882d505bd1b97ab3af7b45eb7e0ebd2dd96f5b`，比发布版多一个文档提交。

本地 `Cargo.toml` 版本为 `1.5.0`，现有未完成合并 `MERGE_HEAD=f55bb64663ba5024ab434017bb6212a0fd9f4bb3` 对应 10 月 3 日。与更新后上游相差 247 个提交、375 个文件；本地已有部分等价修复，提交数量不能理解为全部功能缺失。此次不开展第二轮全量合并、不改版本号、不提交推送。

| 更新项 | 本地兼容性审计结果 |
|---|---|
| v1.9.0 Claude 增强、Code/Cowork 听写、SenseVoice 常驻提速、失败页面修复 | 本轮未移植，非账号问题成因 |
| v1.7.7～1.8.0 供应商按直连/转换/聚合/故障转移拆分、分页保存修复 | 属于新设置和界面架构；不能替换本地自定义聚合编辑器及完整设置保存链 |
| 官方 ChatGPT 登录加入聚合成员 | 与本地“官方认证独立，不参与聚合轮转”契约冲突，未移植 |
| 默认聚合成员发布裸模型名，其他成员用 `cpx-...` | 与本地“官方裸模型直达、第三方必须明确别名”冲突；禁止直接覆盖本地 classifier |
| 默认成员登录同步到 live auth | 与本地独立活动账号、刷新所有权和 CAS 规则冲突；未移植 |
| 上游官方成员 OAuth 刷新 | 进程内锁不能替代本地跨进程租约、桌面端轮换所有权及网络后 CAS；保留本地修复 |
| 聚合采用 multi-agent v1，提示全局 v2 覆盖 | 涉及跨成员加密历史契约；本轮未切换代理协议版本，不以改版本号解决额度归属 |
| 会话关联头透传 | 本轮局部移植安全白名单，未改变账号选择规则 |
| catalog `base_instructions` 必填兼容 | 本轮局部移植，保留已有合法指令 |
| Kimi `tool_search`、Gemini 搜索/推理、原始 apply_patch 和搜索历史兼容 | 已审查提交范围，后续独立移植；`apply_patch` 上游一次改动约 800 行，不能与账号修复混合替换整个代理 |
| 纯 API 显式 bearer 修复 | 本地已有等价实现，无需重复移植 |

完整变更参考 [上游 CHANGELOG 固定版本](https://github.com/BigPizzaV3/CodexPlusPlus/blob/e2882d505bd1b97ab3af7b45eb7e0ebd2dd96f5b/CHANGELOG.md)。上游新增官方聚合模块尚未进入本地运行版，不能把它们当作本次额度问题的原因。Luna 实验性 Ultra、本地 HTTP 目录 `prefer_websockets=false`、外接模型默认 code mode、第三方插件市场及历史 provider 保留均继续保留。

## 本轮修改

1. Renderer 对已知官方裸模型保留显式账号 provider；恢复后的路由记忆记录 model/provider 对，后续无模型回合不会仅根据主目录变为 `openai`。明确从菜单选择主官方来源仍允许回到 `openai`；未知模型和未知来源别名继续拒绝。
2. 真正切换来源成功时提示旧子代理保持各自来源，需要停止并重新创建；旧会话被 fork 时提示同样限制。不静默改写、迁移或终止用户正在运行的子线程。
3. Responses/helper 仅转发三项合法、非空、长度不超过 1024 字节的会话头。供应商显式配置的同名头优先；不转发客户端 Authorization、Cookie 或任意其他头。
4. 专用官方账号请求先校验存档身份，再发送请求；收到响应头后记录 `protocol_proxy.direct_official_dispatch`，包括账号 ID、provider、上游模型、操作、UUID thread/session、状态码和 `willFailover=false`。`phase=response_headers` 与流完成成功分开；取凭据失败或网络尚未获得响应时不记为已分派响应。
5. catalog 缺失或非法 `base_instructions` 时，从同条目的 `model_messages.instructions_template` 回填，否则空串；已有字符串含空串原样保留。
6. 修正两处依赖本机账号目录的 CLIProxy 测试夹具：优先级验证使用明确的目录切片，元数据继承验证使用临时默认设置并精确比较官方 shell 字段。当前账号已移除 `gpt-5.5`，不能用实时账号目录或固定旧版 shell 作为测试前提。仍保留路由顺序、供应商目标和能力断言，不修改生产路由，也不跳过用例。

7. 补齐此前 Luna Ultra 修改遗漏的配置生成测试：Sol 与 Luna 的预期等级都精确包含 Ultra，其余窗口、Fast、Responses Lite 与 multi-agent v2 断言保留；不修改生产目录。

## 验证与发布状态

重点验证已完成：账号 A/B 与相反 live 登录的完整生产分派 mock、Responses/compact、未知来源/模型拒绝、会话头注入防护、helper Authorization 隔离、新 renderer 先红后绿、catalog 先红后绿。

| 验证 | 最终结果 |
|---|---|
| Rust core 全部测试目标 | 1559 通过，0 失败，7 项既有 ignored |
| Rust data 全部测试目标 | 195 通过，0 失败，1 项既有 ignored |
| Manager lib | 101 通过，0 失败 |
| 前端全部测试 | 381 通过，0 失败，1 项既有 skipped |
| renderer 专项 | 101 通过；已包含在上述前端测试中，不重复计数 |
| 格式、workspace check、TypeScript | 均通过 |
| 注入脚本组装检查、git diff --check | 均通过 |

既有 ignored 项涉及浏览器扩展、CDP/本机 E2E 夹具、在线主题仓库及指定真实 rollout 的手动检查；没有为了通过验收而新增 skip。日志与汇总见 `output/provider-audit-20261015/verification-results.json`。本轮未改 Manager React 页面；变更的是 `assets.rs` 通过 `include_str!` 嵌入 Rust 的注入脚本，已使用既有组装器重新生成，不依赖 Vite dist 更新。

Release 工作区构建于 13:34 成功完成，耗时 10 分钟。编译开始前与结束后均核对全部受测源码哈希，构建过程中未变化；Manager 和启动器均验证包含新的账号路由提示函数，启动器另验证包含新的官方出口诊断事件。

13:35:06 已将产物更新到仓库 `bin/` 与实际使用目录 `D:/pro/Codex++`；三处逐项 SHA-256 一致。Manager 和启动器相比旧版均已改变；mobile relay 未涉及本轮代码，内容哈希保持原样，未伪造更新时间。

| 文件 | SHA-256 |
|---|---|
| codex-plus-plus-manager.exe | `205528CA1F78C373C201AAEF06A59AA1749520BA4D8C31E19DED8995E806C96C` |
| codex-plus-plus.exe | `1D3995B75586941BE55DA364FB01A04D61882ECBCAC9BC863C2712FF8200D26A` |
| codex-plus-mobile-relay.exe | `0C22645ADF37A4C65E4462D196A95FEBF5F0347859A391B8A00B75AEEBA3B718` |

运行目录旧文件备份为 `D:/pro/Codex++/backups/provider-account-audit-20261015-133503`，仓库 bin 备份为 `output/provider-audit-20261015/bin-before-20261015-133503`。发布清单见 `output/provider-audit-20261015/deployment.json`，其中明确记录 `restartPerformed=false`。

Manager PID 27804 仍为 10 月 14 日 11:08 启动的旧进程；helper/启动器 PID 12396 为本次构建前 12:37 启动的旧进程。文件更新不代表内存代码已更新。遵循用户此前稍后自行从托盘退出重启的选择，本轮没有结束这些进程，也没有重启桌面端或停止其他聊天。

最终 `git diff --check` 通过。全工作区 scope checker exit 2：包含既有未完成合并、staged/unstaged、untracked、备份与发布产物，共 625 文件，超出阶段预算，不能报告为全树预算通过。本轮相对任务前备份的 12 项已跟踪文件增量另存 `task-numstat-final.json` 与各 `*.final.diff`，阶段追加测试夹具均记录在 gate 中；本轮新增报告单列，不与既有改动混算。没有提交、推送或执行新一轮上游合并。

各阶段的字节备份、gate、任务增量 diff 和日志均保存在 `output/provider-audit-20261015/`。现有未完成上游合并和其他本地改动全部保留；全树 scope 检查须如实展示超限，不能把本轮的局部增量误报为整个工作区满足预算。

## 使用与剩余限制

更新可执行文件不会替换已加载到内存的代码，实际验收须完整退出旧 Manager/Codex++ 后加载新产物。此次不自动结束其他聊天或子代理。

对用户指定聊天，需要先停止旧的 `latency_trace_audit`、`preset_flow_cost`、`draft_visibility_audit` 子代理，再在期望账号来源下重新创建。只修改父聊天或只更新本地程序，不能保证这些已经存在的线程自动改用新账号。

本轮 mock 已验证专用来源按所选账号精确取凭据，官方对应版本源码及历史新建子代理记录支持创建时继承父回合 provider。真实环境的一次父子请求出口验收仍待重启后完成，不能把 mock 当作已发生的线上验收。历史账单无法追溯补算；原生 `openai` 请求不经过 Codex++，不会产生本地专用代理出口日志。
