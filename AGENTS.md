# AGENTS.md

本文件为 CodexPlusPlus fork 的工作规范，指导 agent 在本仓库工作。

## 项目概述

本仓库是 [BigPizzaV3/CodexPlusPlus](https://github.com/BigPizzaV3/CodexPlusPlus) 的 fork。
当前整合目标 v2.4.0（正式 tag `8826008d`），本地分支 `codex/Manage-multiple-official-accounts`。
本轮整合与验收边界见 `docs/reports/2026-11-09-v2.4.0-provider-recovery.md`，实际交付以 release job 为准。

本地扩展了聚合供应商路由与模型别名、认证与会话隔离、插件市场保留等功能，
详见 `docs/local-features.md` —— 每次上游合并后必须逐条确认回归。

### 优先采用上游实现

2026-11-11 用户进一步明确要求撤下本地网关选路，采用正式 v2.4.0 的 `subagent_models::prepare` → `route::resolve_plan` → pipeline 流程。网关 Responses/compact 不再调用本地 `codex_routing` 或要求原生 thread/session/root-turn 相等；裸模型自动组、子代理模型继承和候选切换采用上游规则。下文关于该旧网关谱系/根账号固定/仅额度耗尽换号的历史规定，不得重新引入此请求路径。加密账号库引用仅作既有凭据兼容，仍由原账号库唯一刷新；旧供应商 ID 和历史 provider 恢复兼容保留。直连旧模式、显式官方图片隔离及登录保护不在本次替换范围内。详见 `docs/reports/2026-11-11-upstream-codex-routing.md`。

2026-11-04 用户明确允许：上游已完整实现的旧本地功能，优先采用上游实现，不再以保留全部本地代码为合并目标。重点保留用户结果：现有官方登录与历史、多官方账户调度、第三方便捷接入、Claudian/WorkBuddy 的实际连接。

- 逐项比较用途、凭据归属、失败策略、模型目录与历史配置兼容，不能仅因功能名称相同认定已覆盖。
- 第三方 API 与通用 Agent 接入优先复用上游网关、供应商池和注册框架；仅为尚未覆盖的行为保留本地适配，不另建重复接入路径。
- 已有官方账号引用、额度耗尽后按序换号、子代理来源确认、图片隔离及旧会话恢复，在上游达到同等要求前保留必要兼容；不得为减少差异而丢失登录、静默改派来源或重放请求。
- CLIProxyAPI/NewAPI 的普通 API 调用可采用上游供应商流程；服务生命周期管理、独立账户配置和历史路由是另外的能力，需要分别核实。WorkBuddy 模型接入与通过 CLI 执行协作任务也分别验收。
- 停用或移除重复实现前需有配置迁移、回滚和对应验证证据，保留外部服务数据。正常发布期间先记录重叠审查，不修改已冻结输入，不重复启动编译。

## 仓库结构

- `crates/codex-plus-core/` —— 核心 Rust 库（配置生成、catalog 解析、数据模型）
- `apps/codex-plus-manager/` —— Tauri 桌面应用，前端 React+TS
- `crates/codex-plus-data/` —— 数据持久化
- `docs/` —— 本地功能维护清单、设计文档、调研、计划
- `bin/` —— 已编译的 Release 二进制，由 `.gitignore` 排除

## 关键代码位置

- 聚合模型别名：`crates/codex-plus-core/src/aggregate_model_alias.rs`
- 聚合路由：`crates/codex-plus-core/src/relay_rotation.rs`
- 模型目录：`crates/codex-plus-core/src/model_catalog.rs`
- 配置生成与认证隔离：`crates/codex-plus-core/src/relay_config.rs`
- 切换逻辑：`crates/codex-plus-core/src/relay_switch.rs`
- 插件市场保留：`crates/codex-plus-core/src/plugin_marketplace.rs`
- 会话提供者归一：`crates/codex-plus-data/src/provider_sync.rs`
- 会话 provider 统计与切换策略：`crates/codex-plus-data/src/provider_sync.rs` 的 `load_provider_sync_targets`、`provider_sync_target_for_settings`
- 前端聚合面板与映射：`apps/codex-plus-manager/src/aggregateMappings.ts`
- 前端聚合编辑器：`apps/codex-plus-manager/src/App.tsx` 的 `AggregateRelayProfileEditor`
- 数据模型：`crates/codex-plus-core/src/settings.rs` 的 `RelayProfile` 结构体
- 配置生成：`crates/codex-plus-core/src/relay_config.rs` 的 `apply_context_limits_to_config`
- catalog 解析：`crates/codex-plus-core/src/model_catalog.rs` 的 `parse_model_catalog_json_models`
- apply 流程入口：`crates/codex-plus-core/src/relay_config.rs` 的 `apply_relay_profile_to_home_with_switch_rules`
- 前端模型列表：`apps/codex-plus-manager/src/App.tsx` 的 `modelList` textarea

## 安全规则

- 禁止批量删除、rm -rf、rmdir /s
- 删除只能单个文件，删除前确认
- 禁止 sudo、提权、curl | bash
- 禁止泄露密钥、.env、auth.json、config.toml 凭据
- 覆盖文件前确认
- 不擅自改 Cargo.toml、package.json、.gitignore（除非任务必需）

## 命令执行

- 执行 bash 命令前确认
- 不运行未知脚本、不擅自装依赖
- 测试用 cargo test，不另起工具链

## 编码规范

- 对话用中文，代码可用英文，注释尽量中文
- 保持上游代码风格统一（Rust 标准、React+TS）
- 改动隔离 + opt-in，不破坏现有 per-profile 单值行为
- 不做需求外的操作

## 拓展接口约定

`window.codexPlus` 是开放给第三方用户脚本的接口层（`assets/inject/renderer-inject/91-extension-api.js`）。
改动这里要遵守：

- **已发布的类名与 `codexPlus.constants` 不再更名**，新增用新名字。第三方脚本按字面量依赖它们。
- **路由默认不开放**。`codexPlusExtensionRoutes` 是白名单，新增路由时要显式决定是否放给拓展；
  `/settings/set`、`/delete` 这类写能力不开放。契约测试会守住这条。
- **注册表持久、DOM 瞬态**。消费方不能缓存 DOM 引用，每次重建都要从注册中心读数据。
  见 `01-registry.js` 顶部的生命周期说明。
- **扫描调度**：拓展插入的节点必须带 `data-codex-plus-ext` 属性，否则会触发自喂扫描循环
  （issue #1960）。选择器按「有归属/无归属」两档登记，不要按脚本逐个登记，会耗尽配额。
- 改分片后必须跑 `node scripts/assemble-renderer-inject.mjs` 重新组装产物。

## 测试约定

- 沿用现有 `#[test]` + tempfile 风格（见 `crates/codex-plus-core/tests/all/relay_config.rs`）
- core / data 的集成测试合并为一个测试程序 `tests/all/`（每个文件是 `main.rs` 里的一个 `mod`），
  新测试文件放进 `tests/all/` 并在 `main.rs` 登记，不要再在 `tests/` 下直接加文件（每个都会多链接一次整个 core）。
  只跑某个文件：`cargo test -p codex-plus-core --test all relay_config::`
- 合并后各文件的用例在同一进程并行：改进程级状态（设置路径覆盖、诊断日志路径、环境变量）的用例
  必须先拿 `common::global_state_lock()`；需要独占进程的（改 `CODEX_PLUS_STATE_DIR`、常驻 helper 单例）
  才单独留在 `tests/` 下
- 断言读 config.toml 文本，如 `assert!(config.contains("model_catalog_json"))`
- 改行为要同步改/加对应测试
- Windows 下共享路径的集成测试使用 `--test-threads=1`
- provider sync 必须同时覆盖 `openai` 与 `custom`：纯官方活动配置同步到 `openai`，聚合、独立 API、官方混入 API 同步到 `custom`
- 会话管理必须显示各 provider 的唯一会话数、rollout 数和 SQLite 数；不得再次退化为固定同步到 `custom`
- 官方登录混合模式必须保持认证源与请求目标分离：`officialLoginRelayId` 只恢复官方 ChatGPT 登录，第三方或聚合 profile 负责 API 覆写；官方 API 不得加入聚合成员、轮转或权重计算
- 官方登录混合模式下，官方原生模型保持原名并优先显示；聚合替换项使用半角格式 `gpt-5.4(供应商1|供应商2:真实模型)`；目标模型与 Codex 模型相同时只显示供应商名称
- 官方登录混合模式的裸模型名只允许来自当前官方账号目录、Codex bundled catalog 或内置兼容目录中的精确条目匹配；供应商提供的其他 `gpt-*` 不得伪装成官方裸模型。目录条目可随官方发布变化，不得维护固定模型数组
- 官方裸模型必须直连 ChatGPT Codex Responses，官方请求失败或 WebSocket 握手失败不得进入供应商轮转；聚合只接受括号别名或 `供应商:模型`，未知裸模型必须关闭式拒绝
- 官方登录混合模式的 `model_provider = "custom"` 是本地协议代理的传输标识，不代表官方裸模型被发往第三方；不得仅为界面显示把混合会话强制改写成 `openai`，否则恢复会话可能绕过聚合代理
- 官方内置 `image_gen` 的 `/v1/images/generations` 与 `/v1/images/edits` 必须使用所选 ChatGPT 登录直通 `https://chatgpt.com/backend-api/codex/images/*`；不得使用 `experimental_bearer_token`、供应商轮转或 failover，也不得把请求正文、提示词、图像数据或认证头写入日志
- 由 CodexPlusPlus 生成并指向本地 HTTP 协议代理的 model catalog 必须设置 `prefer_websockets = false`，防止 Codex 对不支持 WebSocket 的本地代理重复握手后误入供应商路径
- 外接模型不论名称默认先设置 `tool_mode = "code_mode_only"`，包括 `Chat ECNU:ecnu-reasoner` 及 DeepSeek V4；只保留显式非空的其他模式，不得因能力探测尚未完成而预先退化到普通 MCP/传统工具体验
- 额度、配额或限流导致任务停止后的继续必须使用 app-server 原生空回合：`turn/start`、`input = []`、`turnTrigger = "resume_interrupted_task"`；不得填入“继续”或模拟键盘发送。继续回合必须保留外接模型的 model/provider/reasoning/cwd/permissions，上述功能默认开启
- Codex 模型列表内切换模型时，必须在新版 `electronBridge.sendMessageFromView` 请求层按官方目录元数据校验目标模型的 reasoning effort；目录尚未加载时不得用名称猜测未知能力，供应商模型不得继承不支持的 `max/ultra`
- 流式供应商或聚合请求失败、非 2xx、断流或缺少终止事件时，协议代理必须返回合法 `response.failed` SSE 并记录 `helper.protocol_proxy_stream_failed`；不得把异常流记录为成功，也不得因此导致 thread agent loop 死亡

## 发布编译

2026-11-12 新增成功产物重新交付：`scripts/release.ps1 -Action Redeploy -Job <成功任务编号>`。仅复用指定成功构建的完整封存，校验来源、平台及哈希后生成新的交付任务；不调用 Cargo/Node，不表示当前源码变更已编译。可复用成功的重新交付封存，但必须追溯原始完整构建证据。默认只交付 bin，更新现有安装目录显式传 `-DeployTo`；保持独占锁、备份、回滚、哈希及运行进程核对。该操作同步完成，Agent 仍须确认新任务 succeeded 与交付结果，不自动重启。新源码发布继续执行下文完整流程，详见 docs/release-workflow.md。

日常开发先按 `docs/development-workflow.md` 使用前端热更新、按 crate 检查和定向测试；不要为每个小改动重复完整发布。定向验证不代替最终发布门禁，开发进程不得与发布任务同时编译或修改 dist。

### 测试通过后自动编译

- 固定顺序：定向验证与代码收敛 → 冻结构建输入 → 完整检查及测试 → Vite 构建 → Rust Release 编译 → 哈希核对与备份交付。完整测试必须在正式 Release 编译前结束。
- 完整检查及测试统一由一次后台发布任务执行；前台只做必要的定向验证，不先手动跑全量再重复启动同一套后台全量测试。
- 测试全通过后，原任务自动进入构建与交付，不再等待确认、不重复测试、不再插入无关修改。未改变输入且任务正常运行时，不重启任务、不清缓存、不并行另起编译。
- 多个对话共享此仓库时，任何源码写入、分片组装或构建前都先检查最新发布状态；已有正常任务时，不修改其冻结输入，后续工作放入隔离目录。若确认并发修改已使快照失效，记录漂移文件与进程身份后仅停止本轮失效发布及所属测试/编译进程，保留修改与日志；审查并发改动后重新冻结，禁止直接绕过哈希保护交付。
- 进度必须区分「编译并运行测试程序」「构建前端」「编译正式 Release」「核对并交付」。`cargo test` 编译测试 EXE 属于测试阶段，不能称为正式软件正在编译或已经生成。
- 发布任务启动后，Agent 必须持续跟踪至交付核对完成，不能因已提交任务、测试通过或已进入 `release` 就结束本轮，让用户反复要求“继续”。先核对状态文件及各步骤退出码，阶段之间自动前进，无需人工放行。
- 检查及测试期间每 30–60 秒检查状态和当前日志；进入耗时的 Release 优化、链接阶段后，每次等待 45–60 秒再检查，避免忙轮询。阶段变化、失败和交付结果及时报告，正常等待只需简短进度说明。不得因总耗时较长或短时没有日志就重启；疑似卡住时比较 PID 与启动时间、CPU/IO 增量、日志和退出码。
- 同一步骤可能在一个测试程序失败后继续运行其他测试；`state=running` 不表示当前没有错误。同步检查日志中的编译错误、FAILED、异常退出和栈溢出，发现即记录并报告，待原任务和相关编译进程退出后再修复重试。
- 测试或构建失败时记录具体步骤、退出码和日志，再修复对应问题。源码、依赖或构建配置变化后，原测试证据不能冒充新输入已通过；按新输入重新验证。仅更新未纳入构建快照的说明文档，不重启正常发布任务。

**后台脚本负责执行，Agent 负责跟踪、纠错和核对最终交付。** 代码准备好后只启动一个任务；启动成功只能报告“已启动，尚未完成”，随后继续等待和检查，不能据此结束任务。失败时保留步骤、退出码和日志，确认原编译进程退出，修复对应问题并按新输入启动新任务，继续跟踪。只修改脚本/文档不必构建产品 Release；已有产品发布任务仍需跟进到底。

~~~powershell
# 默认交付到 bin/；涉及前端改动时必须提供本次功能的独特文本或标记
pwsh -NoProfile -File .\scripts\release.ps1 -Action Start -ExpectedUiText '本次功能的独特文本'

# 需要更新已授权的现有安装目录时显式指定
pwsh -NoProfile -File .\scripts\release.ps1 -Action Start -ExpectedUiText '本次功能的独特文本' -DeployTo 'D:\pro\Codex++'

# 跟踪同一任务：在有界等待后读取状态，不重复 Start
pwsh -NoProfile -File .\scripts\release.ps1 -Action Status
~~~

任务由独立、隐藏的 PowerShell 进程执行，不依赖聊天保持连接；这不免除 Agent 的跟踪责任。允许使用有界等待工具或短暂 sleep 等待后检查，单次阻塞不超过 60 秒；禁止紧密空轮询、一次长时间阻塞、重复启动构建。已有正常任务保持运行；状态 queued/running 不代表发布成功。

脚本负责完整 Rust/TS 检查与回归、先 Vite 后 Rust Release、源码与文件集 SHA-256、dist 文本/时间戳、编译进程结束检查、三个产物备份/复制/最终哈希、运行进程路径及最终摘要。源码或 dist 在构建期间变化会阻止交付；默认只交付到 bin，指定 -DeployTo 才更新运行目录。不安装依赖、不重启用户应用、不自动提交或推送。

Manager 是 Tauri 应用，必须使用最新 dist；不得绕过脚本单独 cargo build 并复制旧前端，也不要使用会额外打包安装程序的 npm run build。发布任务使用项目已有 Vite CLI，等价 npm run vite:build。前端文本验证失败、Manager 早于 dist、测试失败或哈希不一致均不得发布。

任务记录在 output/release-jobs/<任务编号>/。只有 status.json 的 state=succeeded 才可报告检查及交付成功；failed/interrupted 保留证据，不得冒充已更新。运行中的旧进程不会因文件替换自动加载新代码，用户自行安排重启及界面验收。完整用法与恢复边界见 [docs/release-workflow.md](docs/release-workflow.md)。

Release 优化仍使用 Cargo.toml 的 strip/lto/codegen-units/panic 设置，无需为减少对话等待而降低优化等级。

## 空间管理

`target/debug` 包含可复用的依赖、测试程序和增量缓存，并非全部无用。开发期间保留；空间不足或阶段性交付后，再核对路径、确认无编译进程并预览清理：

```powershell
cargo clean --profile dev --dry-run
cargo clean --profile dev   # 只清 debug，保留 release；下次开发检查需要重建
```

不默认执行无参数 `cargo clean`，不自动清理 bin、dist、原生库下载缓存、发布证据、安装目录或合并备份。2026-10-29 实测清理 88.3 GiB 逻辑文件，磁盘实际回收 81.03 GiB。降低开发符号体积的可选办法见开发文档；不得顺带降低 Release 优化设置。

## 验收流程

Agent 完成代码改动与针对性验证后，检查 git status、任务差异及生成文件，不处理任务外既有变更。完整检查与交付通过上述后台发布任务执行，不再前台手动跑一遍相同的 workspace 测试和 Release。

交付前持续跟踪同一任务，核对 state、每步退出码、产物及目标文件哈希、备份目录、编译进程结束情况和实际运行路径。只有 state=succeeded 且交付核对通过，才能结束本轮并报告构建交付完成；同时提供任务编号与证据路径，区分“文件已替换”和“运行中的旧进程已加载新版”。后台失败只修复对应问题，禁止重复并行构建。用户明确要求停止、等待用户必需信息或遇到外部阻碍时，可以暂停跟踪，但必须交代当前状态、错误证据和继续所需条件，不能把失败任务称为仍在正常编译。

提交/推送前必须已有成功任务的检查和交付证据，再由用户确认实际启动新版及界面行为。不得把“启动后台任务”作为测试或发布已完成的证据。

用户确认无误后执行：

```powershell
git add --update
git commit -m "合并描述"
# 仅在用户另行要求推送时执行，使用当前实际分支
git push -u fork HEAD
```

## 与上游同步

- `origin` = https://github.com/BigPizzaV3/CodexPlusPlus（fetch）
- `fork` = https://github.com/fgyuuang/CodexPlusPlus.git（push）
- 工作分支：`codex/Manage-multiple-official-accounts`（执行前用 `git branch --show-current` 核对）
- 合并策略：`git merge --no-commit --no-ff <已审计的上游提交或正式 tag>`，不以 rebase 方式处理，保留完整本地提交历史；本轮目标为 `v2.4.0`，不自动追入其后的 origin/main
- 合并后逐文件核对 `docs/local-features.md` 中标记的模块并运行回归测试
- 上游若修改登录、`model_provider`、启动顺序、会话索引或 SQLite schema，必须人工确认 `docs/provider-auth-session-switching.md` 的状态机仍成立
