# AGENTS.md

本文件为 CodexPlusPlus fork 的工作规范，指导 agent 在本仓库工作。

## 项目概述

本仓库是 [BigPizzaV3/CodexPlusPlus](https://github.com/BigPizzaV3/CodexPlusPlus) 的 fork。
2026-11-19：当前主开发目录为 `D:/pro/codex-plus-plus`，开发主线为 `codex/development-mainline-v2.8.0`，已完成 v2.8.0 合并和发布，用户正在验收。发布输入树为 `636857558fb71e7c7ca88000bd015adb08acf1d3`；与源码无关的后续文档更新不改变该发布证据。上游贡献从独立干净基线拆分，材料见 `output/contributions-v280-20261119/README.md`。下列旧版合并、发布和推送限制为历史记录，以本段和用户当前指令为准。
历史记录：2026-11-18 本轮限源码整合、验证和贡献准备。现行说明见 `docs/reports/2026-11-18-v2.7.2-contribution-audit.md`；上次 2.7.1 的发布证据仍只对应旧输入。未提交的合并不得再次直接 merge/rebase。对外贡献从干净上游基线按功能组织，本分支用于保留个人 fork 历史。

本地扩展了聚合供应商路由与模型别名、认证与会话隔离、插件市场保留等功能，
详见 `docs/local-features.md` —— 每次上游合并后必须逐条确认回归。

### 2026-11-17 上一轮整合边界

用户授权集合所有本地开发成果、核对五条对话并同步最新正式版 `v2.7.1`（`d5dcad3b`），本条覆盖历史章节中的旧目标版本。原分支 `feat/reset-cards-2.6.10` 的 `b8dd3208` 保持不动；当前工作分支已执行 `merge --no-commit --no-ff`，不要在待提交合并上再次 merge/rebase。合并前变化保存在 `output/sync-v271-20261117` 和保留的 stash，不能清理。

本轮不运行 release、不安装依赖、不部署、不重启应用、不动真实登录及外部服务数据、不提交或推送。后续模型先读整合报告与已有日志，按发布工作流接续，不重复本轮审计。WorkBuddy Manager 的压缩/恢复改动位于独立仓库 `D:/pro/workbuddy-manager`，插件的完整管理页也有独立交付边界，不能称为已包含在本仓库二进制中。

管理器采用上游精简入口：增强功能设置在 Codex 中，管理器保留总开关和独立模式补丁；本地 Fast、启动、容量重试、额度继续等开关在 Codex 设置页保留。供应商编辑使用 `relay-profile-editing.ts` 共用实现，先组装 provider-editing 分片，再组装 renderer；注入页复用运行时模型目录，不能打入静态目录中的模型指令。

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

## 注入 Codex 的界面：和 Codex 原生一样

注入到 Codex 里的界面（拓展页、供应商页、任务面板等 `renderer-inject` 分片）要尽量复用 Codex 自己的样式，
做得和 Codex 原生组件一样，减少割裂感。

- **先量原生组件再动手**：在隔离 Codex 实例（独立 `--user-data-dir` + 调试端口）里找到同类原生组件，
  用 CDP 读计算样式和它 class 引用的 CSS 变量（尺寸、圆角、内边距、颜色、阴影、模糊），照着实现，不自己另定一套。
- **不用原生 HTML 控件**：`<select>` 这类控件的弹出层不跟主题，样子也和 Codex 不一样。下拉、菜单复用
  `10-style.js` 里的 `.codex-plus-menu-trigger` / `.codex-plus-menu-popover` / `.codex-plus-menu-item`
  （照 Codex「更改权限」「推理强度」菜单实测），用法见 `41-extension-sources.js` 的排序菜单。
- **颜色、阴影引用 Codex 变量并给兜底**，浅色主题会自动跟着变，例如：
  - `--color-surface-elevated-secondary`：浮层底
  - `--shadow-dropdown-overlay`：已包含 0.5px 描边
  - `--color-token-list-hover-background`：悬停底色
  - `--color-text-tertiary`：次要文字
- **按键在 window 捕获阶段接**：Codex 会在 document 捕获阶段把按键截进输入框。
- **改了 CSS 不用手动加版本号**：`installStyle` 按样式全文哈希判断，同页重新注入也会换成新样式表。
- 交付前把截图和对应的原生组件并排对照。

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

## Claude Desktop 完整功能模式（claude-local，与 ClaudeMom 共用）

完整功能模式的核心不在本仓库：在 [ClaudeMom](https://github.com/BigPizzaV3/ClaudeMom) 仓库的
`crates/claude-local`（本地兼容服务 Node 脚本、证书、真实 profile 切换、启动 / 停止、会话同步、Desktop 探测），
`codex-plus-core` 以 git 依赖按 `rev` 钉住引用它。ClaudeMom 是只做这件事的独立程序，两边只维护这一份。

- Codex++ 这边只有宿主实现 `crates/codex-plus-core/src/claude_local_backend.rs`：设置（`claudeDesktopLocal`）、
  经网关的上游（`claude-desktop-local` 绑定）、常驻 helper 听写、3p 模式。对外函数名不变，管理器不用关心拆分。
- 改完整功能模式的行为（代理脚本、证书、profile、启动流程）去 ClaudeMom 仓库改、测、推，再把这里的 `rev` 升上去。
  升级用 `node scripts/bump-claude-local.mjs`（改 `rev`、更新 `Cargo.lock`、列出带进来的提交）；加 `--check` 只检查是否落后，
  `check-claude-local.yml` 每天跑一次，落后时失败。
  本地联调可在根 `Cargo.toml` 临时加
  `[patch."https://github.com/BigPizzaV3/ClaudeMom"] claude-local = { path = "../ClaudeMom/crates/claude-local" }`，**不要提交**。
- 补丁 Desktop（汉化副本，`claude_localization.rs` + `assets/claude-localization/`）仍在本仓库，Desktop 探测调 claude-local。

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

## 2026-11-12 本轮升级与独立拓展

用户已授权升级正式 v2.6.6 和并行子代理协作。隔离整合目录 D:/pro/codexpp-v266-integration-20261112，原工作区与 v2.4.0 待提交合并保留。独立拓展通过 codex_plus_extensions 工具写入用户包；本轮不删除旧账号库、历史兼容或外部服务。网关仍优先采用上游选路。此授权覆盖前文旧 v2.4.0 目标限制。

## 2026-11-13 最新版与分支修复

用户明确要求修复时追上最新正式版本。本轮核对最新为 v2.6.7，采用已成功构建的 v2.6.6 源码继续合并；不覆盖 C 盘原工作区的待提交合并，不自动提交、推送或重启。历史并行授权不扩展为本轮新的子代理请求。

## 2026-11-16 仓库迁移说明

主开发入口为 D:/pro/codex-plus-plus。2026-11-16 后续历史收敛已将公共 Git 数据库移至主目录自己的 .git，当前目录是独立主工作区，不再依赖旧 2.4.0 目录。历史材料实际集中在 D:/pro/codex-plus-plus/history；旧 D:/pro/codex-plus-plus-history 空壳及旧 v269 联接也已收纳到 history/consolidation-20261116，不再作为外部入口。两个独立插件源码目录已归档于 history/plugins，已安装用户包和外部服务数据未迁移，后续开发或贡献前先提取所需源码。迁移文档中关于 Git 依赖旧目录的描述是已替代的阶段记录。

本次进行了迁移、C/D 盘编译缓存清理、历史工作区归档及 Git 主工作区整理，没有合并、提交或推送。历史章节中的分支名与基线只适用于其原整合任务，后续操作先核对当前实际分支及用户本轮目标。最新目录与保留边界见 history/README.md、docs/reports/2026-11-16-history-consolidation.md；先前迁移与空间审计报告保留阶段证据。

原 C:/Users/fgy/Documents/codex-plus-plus 只保留被 PSForge 工具进程占用的日志及迁移指引，已不是开发仓库。所有仓库操作应显式使用 D 盘主开发目录。既有安全、测试、发布规则继续适用。

用户明确要求仅保留当前开发基线，不再持续复制整套旧源码。history 只保存本地历史材料，不得在其中开发、编译或作为下一轮升级基线；它通过 .git/info/exclude 的 /history/ 排除，不提交或推送其中的备份与私有配置。旧工作区的独有修改保存为恢复补丁及小型压缩包，基准提交由 refs/archive/20261116/* 保护；这不代表维护多条开发路线。迁移重复副本的删除被自动审批阻止，保留在 history/legacy，并记录为待清理，不能报告已删除。
