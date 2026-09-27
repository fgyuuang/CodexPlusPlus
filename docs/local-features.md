# CodexPlusPlus — 本地功能维护清单

本文件记录本 fork 相对于上游 BigPizzaV3/CodexPlusPlus origin/main 的核心差异功能。
每次上游合并后必须逐条确认回归，避免本地行为被上游简化行为覆盖。

---

## 1. 聚合供应商与模型路由（最大分叉）

本地扩展了聚合供应商的模型别名、路由、展示、映射管理和故障切换。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| 聚合模型别名 | `crates/codex-plus-core/src/aggregate_model_alias.rs` | 上游无此文件。成员别名、dispatch entries、catalog 别名全部在这里。合并后如果编译通过不代表逻辑正确。 |
| 聚合路由 | `crates/codex-plus-core/src/relay_rotation.rs` | `classify_mixed_model_route`、`aggregate_member_pool_for_provider_alias`、`dispatch_entries`、aggregate failover 选择逻辑。混合模式必须先区分官方裸模型与供应商别名。上游行为完全不同，合并后需逐函数确认。 |
| 官方直连代理 | `crates/codex-plus-core/src/protocol_proxy.rs` | 裸官方模型从实时 `auth.json` 读取 ChatGPT access token/account id，直连官方 Codex Responses；官方错误只有一个候选，禁止进入 aggregate failover。 |
| 官方图像工具代理 | `crates/codex-plus-core/src/protocol_proxy.rs`、`launcher.rs` | 混合模式下把 Codex 内置 `gpt-image-2` 的 generation/edit 请求直通 ChatGPT Codex Images；不使用第三方 key，不进入聚合轮转。 |
| 模型目录 | `crates/codex-plus-core/src/model_catalog.rs`、`aggregate_model_alias.rs`、`model_suffix.rs`、`official_model_catalog.rs` | `displaySuffix` 注入、当前官方目录优先排序、提供者独立模型条目（`供应商:模型`）生成；所有外接模型默认先使用原生 `code_mode_only`，只有当前账号目录、bundled catalog 或兼容目录中精确存在的官方条目才继承 reasoning、Fast、工具与基础指令，未知模型不得冒充官方能力；默认模型不得被供应商专属首项抢占。 |
| 聚合数据结构 | `crates/codex-plus-core/src/settings.rs` | `AggregateRelayProfile`、`AggregateRelayMember`、`AggregateRelayModelMapping`、`AggregateRelayDispatchTarget`。 |
| 前端聚合面板 | `apps/codex-plus-manager/src/aggregateMappings.ts` | 新文件。展示顺序、有效映射计算、提供者标签生成；列表按当前官方目录顺序把官方模型放在前面，供应商模型按成员顺序在后。 |
| 前端聚合编辑器 | `apps/codex-plus-manager/src/App.tsx` | `AggregateRelayProfileEditor`、`normalizeAggregateConfig`、`inferAggregateModelList`、`aggregateDisplayModelEntries`。 |
| 前端测试 | `apps/codex-plus-manager/src/aggregateMappings.test.ts` | 顺序回归测试。 |

**合并确认点**：检查聚合供应商保存→应用后，模型下拉是否出现带括号的正确名称、官方模型是否按当前官方目录顺序排列、供应商模型是否按成员顺序排列、默认模型是否不再落到供应商专属首项、mappings 编辑是否可保存恢复。必须额外模拟官方请求失败，确认供应商端口没有收到请求；供应商模型只能显示为括号别名或 `供应商:模型`。使用可信官方别名或 `Chat ECNU:ecnu-reasoner` 等非 GPT 外接模型打开 `codex://threads/...` 时，都应先走原生 `exec`/`codex_app.read_thread` 路径，不得预先退化为反复探测普通 MCP 服务器 `codex`；端点不支持 custom tool 时再返回明确失败。

官方模型目录由 `official_model_catalog.rs` 维护：Manager 启动、Codex 启动/重启、供应商切换和官方账号切换前尝试刷新当前账号的 `/backend-api/codex/models`。缓存按账号保存于 `.codex-session-delete/official-model-catalog.json`，只记录模型条目、ETag、客户端版本和时间戳，不保存令牌或认证头。请求失败时依次回退到当前账号快照、bundled CLI、仓库兼容目录，不能阻止 Codex 启动；`get_official_model_catalog_status` 与 `refresh_official_model_catalog` 只返回安全状态和可见模型列表。官方目录或 CLIProxyAPI `/v1/models` 刷新成功后，若 `managed-cliproxy-official` 已启用，Manager 必须按当前可信目录顺序重新筛选该 profile；模型集合变化时同步重建当前活动混合聚合 catalog，但不得启动 CLIProxyAPI、关闭 Codex 或触发 Codex 重启。

---

## 2. 认证与会话隔离

非官方供应商的 API key 写入 `experimental_bearer_token` 而非 `auth.json`，
切换供应商时不覆写官方 ChatGPT/Codex 登录态。

本地还提供全局“官方登录混合模式”：选定一个官方登录 profile 作为认证源，随后可以直接切换独立 API 或聚合供应商作为请求目标。官方认证先恢复，第三方 bearer token 后覆写请求；官方 API 永远不加入聚合成员池。官方原生模型保持原名并优先显示，聚合替换模型追加显示为 `gpt-5.4(供应商1|供应商2:真实模型)`。裸官方模型通过本地代理独立转发到 ChatGPT Codex Responses，失败时不会轮转到第三方。内置 `image_gen` 的 `gpt-image-2` generation/edit 请求同样使用官方 ChatGPT 登录直通官方 Images 端点。

**第三方模型必须由 Codex++ 调度转发**：根 `model_provider` 写成 `"openai"` 时，如果 `config.toml` 里没有 `[model_providers.openai]` 段，Codex 用的是**内置官方 provider**，请求会**直连 chatgpt.com**，完全绕过本地协议代理；这时选中 `deepseek:*`、`Chat ECNU:*` 等非官方模型会被账号侧拒绝，报 `The 'X' model is not supported when using Codex with a ChatGPT account.`（该文案不在任何本地二进制里，来自服务端）。把非官方模型送到代理的唯一途径是注入层给会话套上路由 provider：`renderer-inject.js` 的 `patchCodexModelReasoningRequestMessage()` 必须覆盖**新版桌面端的裸 JSON-RPC 形态**（`{ method, params }`，没有 `send-cli-request-for-host` / `start-turn-for-host` 这类 `type`），否则 `modelProvider` 始终为空。判据：诊断日志里 `remote_session_provider_override_applied` 是否出现、`protocol_proxy.*` 是否有流量。该补丁不得只挂在“模型白名单解锁”一个开关上——那会让关掉解锁的用户静默退回直连官方。改这个脚本后必须重新 `cargo build --release` 并部署（`assets.rs` 用 `include_str!` 编译进二进制）。

当前“官方体验多来源路由”以主官方账号为默认入口。普通第三方模型只从当前选中的供应商生成；选中聚合供应商时，只从该聚合的成员和有效映射生成模型，不能使用聚合 profile 的缓存 `modelList` 或其他未选中供应商。旧能力绑定必须重新匹配当前来源、模型和路由别名才能进入目录。能力模板映射在各供应商详情内管理，CLI 通用模型在 CLIProxyAPI 页面内管理；无同名官方模板的模型需显式绑定后才显示。

官方体验目录写在 CODEX_HOME 根目录 `codex-plus-official-experience-model-catalog.json`，与 `model-catalogs/<id>.json` 同属受管产物：
`is_codex_plus_managed_model_catalog` 必须按文件名（`codex-plus-*-model-catalog.json`）一并识别，否则切换回聚合/独立供应商时会把官方体验的扩展模型复制进目标 profile 的 catalog。那些条目的 `provider` 只存在于官方体验 config 里，复制过去会让模型列表出现“能选中但发不出去”的僵尸条目，同时把聚合自己的模型挤出目录。

账号额度用尽后桌面端会锁死输入区（`account/rateLimits/read` 的 `allowed/limit_reached/rate_limit_reached_type/spend_control.reached/credits.overage_limit_reached` 任一命中即视为受限，`credits.has_credits=false` 且 `unlimited=false` 也会兜底判定耗尽），此时换模型也无法继续任务。开了“额度继续”（`quotaResume`，默认开启）时，注入层必须同时做两件事：在额度响应传输层把上述阻断标志改写为未触达（保留真实用量字段），并在 DOM 层解除 `inert`/`pointer-events`/禁用属性。服务端额度判定不受影响，改写只解除客户端阻断。改注入层逻辑时必须同步提升 `codexRateLimitUnlockVersion` 并更新 `apps/codex-plus-manager/src/rateLimitUnlock.test.ts`。

新版 ChatGPT 桌面端（Store 26.924.2738.0 及之后）**不再通过 app-server 的 `account/rateLimits/read` 取限额状态**，而是走 `kW` 传输单例的 HTTP `/wham/usage` 与 SSE `/wham/usage/stream`（`usage.snapshot` 事件），再写入 React Query 的 `rate-limit-status` 缓存。只拦截 app-server 会导致输入框依旧灰掉。注入层必须三路覆盖：

- `patchCodexUsageHttpTransport(module)`：遍历动态 import 的模块导出定位 `kW` 单例（`fetch`/`stream` 方法 + `streamControllers: Map` + `stream` 源码含 `fetch-stream-event`），包装其 `fetch` 只改写 `/wham/usage` 的成功 JSON 响应；包装 `stream` 时包一层 `onEvent`，收到 `usage.snapshot` 就改写 `event.data.usage`。
- `refreshCodexUsageLimitQueryCache()`：清理注入前已进入 React Query 的 `rate-limit-status` 数据，否则旧快照仍锁着输入区。
- 保留 `account/rateLimits/read` 的旧通道，覆盖尚未升级的客户端。

Microsoft Store 更新可能在 Codex++ launcher 仍运行时关闭原窗口，并另起没有 CDP 调试端口的新版本。此时 57321 代理和 launcher 都还活着，但注入已失效，模型切换与额度解锁均无法进入新窗口。接管重启（`prepare_codex_launch` → 停止无 CDP 的桌面端再拉起）只允许发生在两个用户显式发起的路径：主 launcher 的首次启动（`launch_and_inject_with_hooks`）与 Manager 的显式重启（`restart_codex_plus`，先停止再拉起）。**单实例复用路径 `activate_existing_codex_app` 严禁调用 `prepare_codex_launch`**：它会在用户无感知时杀掉正在使用的 ChatGPT 窗口，表现为“Manager 反复唤起 ChatGPT 并关闭我的窗口”。该路径只做窗口激活与注入状态检查，接管交给用户显式点击的启动/重启。

**桌面端 GUI 宿主绝不能被当作 CLI 执行。** Store/MSIX 包内的 `ChatGPT.exe`（便携版为 `Codex.exe`）只能通过包激活（AUMID）启动；直接 spawn 出来的进程没有程序包标识，新版桌面端会在 bootstrap 阶段失败（日志 `Desktop bootstrap failed to start the main app phase=bootstrap-import-main`、`[sparkle] Failed to set up updater 该进程没有程序标识符`），并弹出 `ChatGPT failed to start.` 对话框，窗口永远不出现。因此 `official_model_catalog.rs` 的 `codex_cli_candidates` 必须过滤掉 GUI 宿主（`is_windows_desktop_gui_executable`）：`ChatGPT.exe` 忽略大小写匹配，`Codex.exe` **必须精确匹配大小写**，因为包内真正的 CLI 是小写的 `app/resources/codex.exe`。同一时刻只允许存在一种启动方式——CLI 探测只准用包内 `resources/codex.exe` 与 `%LOCALAPPDATA%\OpenAI\Codex\bin\*\codex.exe`。该路径会在 Manager 启动时（`refresh_official_model_catalog_in_background`）被触发，历史上表现为“一开 Manager 就弹 ChatGPT 启动报错”。

**Codex++ 自身不得开机自启动桌面端。** `CodexPlusPlusWatcher.lnk`（启动目录）与 `CodexPlusPlusWatcher`（`HKCU\...\CurrentVersion\Run`）都会在登录时以 `--debug-port` 拉起 launcher 进而启动 Codex。用户要求关闭自启动时，除了移除这两个入口，还要写 `~/.codex-session-delete/watcher.disabled`，否则 `install_watcher` 会再次安装。

**只有直连官方账号的 route provider 可以用 ChatGPT 账号鉴权。** `ensure_official_experience_config_in_home` 为每个路由来源写 `[model_providers.<provider_id>]`；`requires_openai_auth` 必须**仅对 `OfficialExperienceSourceKind::DirectOfficial` 为 true**。一旦聚合（`codex_plus_aggregate_*`）、CLIProxyAPI（`codex_plus_cli_*`）等非官方通道也被写成 `requires_openai_auth = true`，Codex 会把会话当作 ChatGPT 账号会话并按账号限制模型名，用户选中 `Chat ECNU:ecnu-reasoner`、`deepseek:*` 等条目时会直接报 `The 'X' model is not supported when using Codex with a ChatGPT account.`（该文案不在本地任何二进制里，来自账号校验链）。非官方通道改为 `requires_openai_auth = false` + `experimental_bearer_token = NO_AUTH_PROXY_BEARER_TOKEN`；协议代理不校验 routes 请求的 bearer，官方上游凭据由代理自己从登录存档解析（`resolve_official_chatgpt_auth`）。

**同步流程不得改写用户在目录内的模型选择。** `ensure_official_experience_config_in_home` 只应把**目录外的未知模型**回退为官方默认模型；目录内出现的 `routing_slug`（官方账号别名、CLIProxyAPI、聚合、`供应商:模型`）都是合法选择，必须原样保留，否则用户选了第三方模型会在下一次配置同步时被静默改回官方模型。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| 配置写入 | `crates/codex-plus-core/src/relay_config.rs` | `requires_openai_auth` 识别、token 路径选择、`save_relay_file` 拦截写 auth。 |
| 切换逻辑 | `crates/codex-plus-core/src/relay_switch.rs` | `save_backfill_profile_for` 参数变更，不再从 live config/auth 回填之前 provider。`backfill_relay_profile_from_live` 已移除。 |
| Manager 后端 | `apps/codex-plus-manager/src-tauri/src/commands.rs` | `save_relay_file` 限制、`sync_providers_now` 参数简化。 |
| 启动器 | `apps/codex-plus-launcher/src/main.rs` | 启动时调用会话归一。 |
| 混合模式设置 | `crates/codex-plus-core/src/settings.rs`、`apps/codex-plus-manager/src/App.tsx` | `officialLoginMixedMode`、`officialLoginRelayId`；官方账号与实际请求目标分开选择。 |
| 混合模式应用 | `crates/codex-plus-core/src/relay_switch.rs`、`relay_config.rs` | 先恢复官方登录，再原子写入第三方/聚合配置；聚合成员不包含官方 API。 |
| 官方账号库 | `crates/codex-plus-core/src/official_accounts.rs` | 多账号身份去重、DPAPI/本地文件凭据保护、OAuth/设备码、按需令牌刷新与用量刷新、凭据新旧判定、加密导入导出、旧 profile 迁移。 |
| Manager 账号维护 | `apps/codex-plus-manager/src-tauri/src/commands.rs`、`apps/codex-plus-manager/src/App.tsx` | 独立账号列表、元数据维护、显式切换、运行中重启确认；凭据不返回前端。 |
| Codex 注入页账号监控 | `crates/codex-plus-core/src/routes.rs`、`assets/inject/renderer-inject.js` | 左上角 Codex++ 的“官方账号”页按紧凑列表展示完整邮箱、套餐、请求账号/本机登录状态、主次额度与重置时间；桥接只返回安全摘要，刷新不返回令牌，切换进入 Manager 原有重启确认流程。 |

**合并确认点**：普通纯 API 切换后 `auth.json` 不应被覆写且 `requires_openai_auth = false`；官方登录混合模式下必须先从 `activeOfficialAccountId` 恢复所选官方 `auth.json`，随后第三方/聚合配置应同时包含 `experimental_bearer_token` 与 `requires_openai_auth = true`，官方 API 不得进入 aggregate members。多账号切换前只允许较新的 live 凭据回存，旧 refresh token 不得覆盖账号库；身份或同版本令牌冲突不得静默覆盖。`/v1/images/generations` 与 `/v1/images/edits` 必须使用官方认证且官方失败不得连接任何供应商。

### 2.1 CLIProxyAPI 独立接入

CLIProxyAPI 固定部署到 `D:\pro\CLIProxyAPI`，独立负责账号登录、OAuth 刷新、额度、轮转和账号文件。Codex++ 只管理受管服务进程并调用 `/healthz`、`/v1/models`、`/v1/responses`，禁止读取、转换或同步 CLIProxyAPI 的账号目录。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| Manager 服务控制 | `apps/codex-plus-manager/src-tauri/src/cliproxy.rs` | 固定版本下载与 SHA-256 校验、DPAPI 连接密钥、PID/可执行路径核验、独立进程启停。不得并入 `official_accounts.rs`。 |
| CLIProxy 自启动 | `apps/codex-plus-manager/src-tauri/src/cliproxy.rs`、`src/lib.rs`、`src/App.tsx` | `cliproxy-integration.json` 的 `autoStart` 仅控制正常 Manager 启动时的后台服务启动；`--transient` 不触发。启动、停止、重启共用互斥锁，失败只写诊断日志，不读取或同步 CLIProxyAPI 账号目录。Codex 本体不得随 Manager 自动启动。 |
| 受管供应商标识 | `crates/codex-plus-core/src/settings.rs` | `RelayProfile.integrationType = "cliproxy"` 用于识别受管通用直连配置；它不成为聚合成员，也不参与账号同步。`cliproxy-official` 只表示第二开关启用的官方模型专用通道。 |
| 独立模型路由 | `crates/codex-plus-core/src/aggregate_model_alias.rs`、`model_catalog.rs`、`relay_rotation.rs`、`relay_config.rs`、`assets/inject/renderer-inject.js` | CLIProxyAPI 模型使用 `CLIProxyAPI:模型名` 直连受管配置，不进入聚合轮转。按钮2开启时官方模型由 `cliproxy-official` 接管，通用通道只展示非官方模型；只有官方目录精确匹配的 CLI 模型按基础条目继承 Fast 与 reasoning 档位。 |
| Manager 页面 | `apps/codex-plus-manager/src/App.tsx` | 展示状态、API Base URL、连接密钥、模型与测试结果；普通供应商编辑器不得改写或删除受管字段。 |
| 独立配置 | `D:\pro\CLIProxyAPI\config\config.yaml` | 仅首次缺失时生成；已有文件不覆盖。账号维护通过 CLIProxyAPI 的 `/management.html` 完成。 |

**合并确认点**：CLIProxyAPI 默认不自启动；用户可在 Manager 中显式开启随正常 Manager 启动的后台自启动，`--transient` 不触发。Codex 本体不得随 Manager 自动启动。未安装或未启动 CLIProxyAPI 时原功能不受影响；受管供应商 ID 固定为 `managed-cliproxy`，API Base URL 必须包含 `/v1`。官方体验目录先显示主官方 GPT 系列，再显示 `CLIProxyAPI:gpt-*` API 反代模型，然后显示聚合与 ECNU 等第三方模型；CLIProxy 非 GPT 模型排在这些模型后。CLIProxy 官方 API 模型显示完整路由名 `CLIProxyAPI:模型名`，直接切换官方登录账号仍使用裸官方模型名。扩展目录条目的 priority 必须晚于所有官方条目，避免复制能力模板时把外接模型插入官方系列中间。CLI 模型必须通过受管配置直连，禁止加入聚合成员、轮转或 failover。CLIProxy 官方模型只在基础 slug 与当前官方目录精确匹配时继承 Fast、默认 reasoning 和受支持档位；Gemini、普通供应商同名模型及未知模型不得继承这些官方能力，但仍默认先尝试 `code_mode_only`。CLIProxyAPI 账号文件变化不得触发 Codex++ 凭据写回、额度刷新或 provider sync。

### 2.2 NewAPI 独立接入

NewAPI 保持独立部署并自行维护渠道、用户、令牌、数据库、配额和内部调度。Codex++ 只通过可配置的 Docker Compose 项目路径执行启停，调用 `/api/status`、`/v1/models` 与 `/v1/responses`，并把 NewAPI 保存为一个普通受管 API 供应商；不得读取数据库、管理员会话、渠道密钥或 Compose 环境变量。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| Manager 服务控制 | `apps/codex-plus-manager/src-tauri/src/newapi.rs` | 项目目录、Compose 文件、Docker 可执行文件、API 服务名和 Base URL 均可配置；启动使用 `up -d`，停止使用 `stop`，禁止执行 `down` 或删除卷。 |
| 连接凭据 | `apps/codex-plus-manager/src-tauri/src/newapi.rs` | Manager 的用户 API Token 副本使用当前 Windows 用户 DPAPI 保存；不得持有 root/admin cookie，也不得在错误、状态或诊断日志中输出 Token、Authorization 头、请求正文或 Compose 内容。 |
| 受管供应商 | `apps/codex-plus-manager/src-tauri/src/newapi.rs`、`crates/codex-plus-core/src/settings.rs` | 稳定 ID 为 `managed-newapi`，`integrationType = "newapi"`，协议为 Responses、模式为 Pure API。NewAPI 是普通供应商，不使用 CLIProxyAPI 的官方模型特殊路由。 |
| Manager 页面 | `apps/codex-plus-manager/src/App.tsx` | 独立显示 Docker/Compose/API 状态，提供启停、控制台、渠道页、令牌页、模型刷新、API 测试和供应商接入。普通供应商编辑器不得改写或删除受管字段。 |

**合并确认点**：`D:\pro\newapi` 仅为首次默认值，运行时必须允许修改；Base URL 与本地 Compose 路径相互独立，以便只连接远程 NewAPI。Manager 退出后 Compose 服务继续运行。不得自动拉取镜像、覆盖 Compose 或更新本地定制二进制。`/api/status` 必须验证 `success = true` 和 NewAPI 数据结构；模型 ID 从 `/v1/models` 原样保存。关闭接入只删除 `managed-newapi` 供应商并清理聚合引用，不停止 NewAPI 服务或修改其数据。

### 2.3 容量错误代理内重试

开启“capacity 重试”后，官方登录、普通供应商和聚合供应商返回的模型容量错误都由 Codex++ 协议代理捕获。代理保持 Codex 的原请求连接，在内部按 Manager 自定义次数重新发起请求；中间 `error`、`response.failed` 或 HTTP 错误不得写入 Codex Responses 流，因此不进入 agent loop，也不占用 Codex 自身的错误计数。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| 容量识别与重试 | `crates/codex-plus-core/src/protocol_proxy.rs` | 解析 SSE/JSON 的 `error.message/code/type` 与 `response.failed`，等待真实输出或终止事件后再决定放行；不得恢复固定 15 秒探测窗口。 |
| 协议代理连接 | `crates/codex-plus-core/src/launcher.rs` | 重试在原本地 HTTP 连接内完成，只在自定义次数耗尽后传递最后一次原始容量错误。`/backend/status` 只发布不含请求内容的带外重试状态。 |
| Codex 界面提示 | `assets/inject/renderer-inject.js` | 通过带外状态显示“模型容量不足，正在重试”，不得为了提示而向 Codex 注入 `response.failed`、合成 503 或错误 SSE。 |
| Manager 设置 | `apps/codex-plus-manager/src/App.tsx` | “错误与重试”提供开关和 1–20 次自定义次数；次数只统计 Codex++ 内部重发。 |

**合并确认点**：使用含 `response.created` / `response.in_progress` 前导事件、延迟容量失败、`model_at_capacity` 代码和 JSON 转义消息的流式用例验证。命中后日志必须出现 `protocol_proxy.capacity_retry_loop`，Codex 页面可以显示重试提示，但任务不得收到中间失败事件；成功重试后同一任务继续执行。

### 2.4 额度停止后的原生空回合继续

“额度停止后继续”默认开启。任务因 usage limit、quota、rate limit、billing limit、credits exhausted、HTTP 429 或对应中文额度错误停止后，如果当前输入为空且没有运行中的回合，Codex++ 在原发送按钮位置显示三角继续按钮。点击后直接调用 app-server `turn/start`，传入 `input: []` 和 `turnTrigger: "resume_interrupted_task"`，不得向输入框写入“继续”、模拟 Enter 或创建普通文本回合。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| 设置与页面 | `crates/codex-plus-core/src/settings.rs`、`apps/codex-plus-manager/src/App.tsx` | `codexAppQuotaResume` 默认开启，受“启用 Codex增强”总开关控制。 |
| app-server 状态机 | `assets/inject/renderer-inject.js` | 记录最近一次 `turn/start` 的 cwd、权限、模型、provider、service tier、reasoning、personality 和 collaboration mode；只保存恢复参数，不保存原输入正文。 |
| 继续按钮 | `assets/inject/renderer-inject.js` | 监听 `turn/started` / `turn/completed`；额度类 `failed` 才显示自定义按钮，运行中、输入非空、普通错误或正常完成不得显示。 |
| 回归测试 | `apps/codex-plus-manager/src/renderer-inject.test.ts`、`crates/codex-plus-core/tests/capacity_retry_settings.rs` | 覆盖额度识别、ECNU/DeepSeek provider 参数保留、空 `input`、默认设置和禁止文本“继续”。 |

**合并确认点**：使用外接 ECNU DeepSeek V4 制造额度不足停止，确认输入为空时发送按钮替换为三角继续按钮；点击后 app-server 收到同 thread 的空 `turn/start`，`turnTrigger` 为 `resume_interrupted_task`，模型、provider、reasoning、cwd 与权限不变，rollout 中不得新增用户文本“继续”。恢复回合开始后按钮必须立即退出继续态；若再次额度失败则重新出现。

---

## 2.5 Codex App MCP 与 Agent 工具兼容

Codex 0.154+ 的宿主任务工具优先通过内置 `codex_app` MCP 提供。Codex++ 保留 `CLIProxyAPI:模型名` 路由标识，同时保护内置插件/MCP 配置、记录安全的工具装配探针，并在本地 Responses 代理发现新旧入口同时存在时过滤已废弃的 `codex_app` dynamic namespace。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| 工具探针与版本判定 | `crates/codex-plus-core/src/codex_app_tools_compat.rs` | 只记录 app-server、model/provider、MCP 是否出现、工具数量和旧入口冲突；禁止记录 prompt、参数、正文和凭据。 |
| Responses 兼容过滤 | `crates/codex-plus-core/src/protocol_proxy.rs` | 仅在 `mcp__codex_app` 与旧 `codex_app` 同时出现在 `tools` 时移除旧入口；旧入口单独存在时保持原样，其他动态工具和 MCP 不受影响。 |
| 插件配置保留 | `crates/codex-plus-core/src/relay_config.rs` | profile 应用不得删除 `codex-app-tools@openai-bundled`、`mcp_servers.codex_app` 或用户显式工具审批配置。 |
| Manager 诊断 | `apps/codex-plus-manager/src-tauri/src/commands.rs`、`apps/codex-plus-manager/src/App.tsx` | 显示 Agent 工具状态，区分 MCP 可用、旧入口冲突、MCP 缺失和待新建任务。 |

**合并确认点**：模型来源只改变 Responses 传输目标，不得减少 `codex_app`、`exec`、`apply_patch`、Skills、普通 MCP 或审批流程；必须用新建任务验证 `read_thread` 实际通过 `mcp__codex_app` 成功。Codex 版本升级后先看探针状态，再决定是否保留兼容过滤。

---

## 3. 会话提供者统计与自适应归一（provider sync）

历史会话可以按当前使用入口归一：纯官方登录使用 `openai`，聚合、独立 API 和官方混入 API 使用 `custom`。
Manager 会列出从 config、rollout、SQLite 发现的 provider，并显示唯一会话数、rollout 数和 SQLite 数；用户也可手动选择同步目标。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| 核心逻辑 | `crates/codex-plus-data/src/provider_sync.rs` | `run_provider_sync_with_target()`、`load_provider_sync_targets()`、`provider_sync_target_for_settings()`、唯一会话计数与备份机制。 |
| Manager 后端 | `apps/codex-plus-manager/src-tauri/src/commands.rs` | `sync_providers_now(target_provider)` 接收显式目标，不得固定为 `custom`。 |
| Manager 前端 | `apps/codex-plus-manager/src/App.tsx` | 会话页展示 provider 统计、当前配置 provider 和目标选择器。 |
| 启动器 | `apps/codex-plus-launcher/src/main.rs` | 启动前自动同步跟随活动供应商模式。 |
| 测试 | `crates/codex-plus-data/tests/provider_sync.rs` | 覆盖 rollout/SQLite/backup/rollback、provider 统计和官方/custom 目标策略。 |
| 公开 API | `crates/codex-plus-data/src/lib.rs` | 暴露 provider_sync。 |

**合并确认点**：纯官方入口启动前目标必须为 `openai`；聚合/独立 API 入口目标必须为 `custom`。Manager 必须同时显示 provider 数量和允许显式选择目标。rollout 与 SQLite 写入前备份，锁定 rollout 必须跳过并报告。

详细状态机与限制见 `docs/provider-auth-session-switching.md`。

---

## 4. 插件市场保留

openai-curated/remote marketplace 配置写入时保留已有的第三方 marketplace 条目。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| 合并逻辑 | `crates/codex-plus-core/src/plugin_marketplace.rs` | `merge_marketplace_entries_with_locally_selected`、`keep_openai_curated_and_custom_entries`。 |
| 注入脚本 | `assets/inject/renderer-inject.js` | 插件自动展开关闭、backend settings 加载时序。 |

**合并确认点**：保存聚合供应商后，已有第三方 marketplace 插件配置不丢失。

---

## 5. 模型窗口与上下文窗口

源自上游 v1.2.41+ 的 feature，本地做了兼容修复。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| 前端辅助 | `apps/codex-plus-manager/src/model-windows.test.ts` | modelWindows / modelVlm 字段完整性测试。 |
| 目录生成 | `crates/codex-plus-core/src/relay_config.rs` | `apply_context_limits_to_config`、`suffix stripping`、`model_catalog_json` 生成。 |

---

## 6. 其他本地调整

- `.gitignore` 增加 codex agent 忽略规则
- `Cargo.toml` 增加 `mobile-relay` workspace member
- 编译配置：`[profile.release]` 增加 `strip = true`、`lto = "fat"`、`codegen-units = 1`、`panic = "abort"`（减少二进制体积约 35%）
- `provider_import.rs`、`ccs_import.rs` 的细微兼容修补

---

## 上游合并流程

```powershell
# 1. 拉取上游
git fetch origin --prune

# 2. 以本地为主进行三方合并（非自动 rebase）
git merge --no-commit --no-ff origin/main

# 3. 逐模块确认以下文件未被上游覆盖
#    - crates/codex-plus-core/src/aggregate_model_alias.rs          （上游无此文件）
#    - crates/codex-plus-core/src/relay_rotation.rs                 （aggregate 函数）
#    - crates/codex-plus-core/src/model_catalog.rs                  （catalog 别名注入）
#    - crates/codex-plus-core/src/relay_config.rs                   （认证隔离）
#    - crates/codex-plus-core/src/relay_switch.rs                   （backfill 移除）
#    - crates/codex-plus-core/src/official_accounts.rs              （官方多账号库与加密凭据）
#    - crates/codex-plus-core/src/plugin_marketplace.rs             （保留逻辑）
#    - apps/codex-plus-manager/src/aggregateMappings.ts             （新文件）
#    - apps/codex-plus-manager/src/App.tsx                          （聚合面板）
#    - apps/codex-plus-manager/src-tauri/src/commands.rs            （save_relay_file 拦截）
#    - crates/codex-plus-data/src/provider_sync.rs                  （会话归一）

# 4. 运行关键测试
cargo test -p codex-plus-core --tests -- --test-threads=1
cargo test -p codex-plus-data --tests -- --test-threads=1
cargo test -p codex-plus-manager --lib -- --test-threads=1

# 5. TypeScript 检查与前端测试
cd apps/codex-plus-manager
../node_modules/.bin/tsc --noEmit -p tsconfig.json
node --test "src/*.test.ts"

# 6. 发布编译
cargo build --workspace --release
