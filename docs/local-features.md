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
| 模型目录 | `crates/codex-plus-core/src/model_catalog.rs`、`aggregate_model_alias.rs` | `displaySuffix` 注入、官方模型优先排序、提供者独立模型条目（`供应商一:gpt-5.4`）生成；默认模型不得被 `composer-2.5` 等供应商专属首项抢占。 |
| 聚合数据结构 | `crates/codex-plus-core/src/settings.rs` | `AggregateRelayProfile`、`AggregateRelayMember`、`AggregateRelayModelMapping`、`AggregateRelayDispatchTarget`。 |
| 前端聚合面板 | `apps/codex-plus-manager/src/aggregateMappings.ts` | 新文件。展示顺序、有效映射计算、提供者标签生成；列表固定为官方 `gpt-5.6-sol/terra/luna` 等模型在前，供应商模型按成员顺序在后。 |
| 前端聚合编辑器 | `apps/codex-plus-manager/src/App.tsx` | `AggregateRelayProfileEditor`、`normalizeAggregateConfig`、`inferAggregateModelList`、`aggregateDisplayModelEntries`。 |
| 前端测试 | `apps/codex-plus-manager/src/aggregateMappings.test.ts` | 顺序回归测试。 |

**合并确认点**：检查聚合供应商保存→应用后，模型下拉是否出现带括号的正确名称、官方模型是否按 `5.6-sol → 5.6-terra → 5.6-luna → 其余模型` 排列、供应商模型是否按成员顺序排列、默认模型是否不再落到 `供应商:composer-2.5`、mappings 编辑是否可保存恢复。必须额外模拟官方请求失败，确认供应商端口没有收到请求；供应商的 `gpt-5.2` 等模型只能显示为括号别名或 `供应商:模型`。

---

## 2. 认证与会话隔离

非官方供应商的 API key 写入 `experimental_bearer_token` 而非 `auth.json`，
切换供应商时不覆写官方 ChatGPT/Codex 登录态。

本地还提供全局“官方登录混合模式”：选定一个官方登录 profile 作为认证源，随后可以直接切换独立 API 或聚合供应商作为请求目标。官方认证先恢复，第三方 bearer token 后覆写请求；官方 API 永远不加入聚合成员池。官方原生模型保持原名并优先显示，聚合替换模型追加显示为 `gpt-5.4(供应商1|供应商2:真实模型)`。裸官方模型通过本地代理独立转发到 ChatGPT Codex Responses，失败时不会轮转到第三方。内置 `image_gen` 的 `gpt-image-2` generation/edit 请求同样使用官方 ChatGPT 登录直通官方 Images 端点。

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

**合并确认点**：普通纯 API 切换后 `auth.json` 不应被覆写且 `requires_openai_auth = false`；官方登录混合模式下必须先从 `activeOfficialAccountId` 恢复所选官方 `auth.json`，随后第三方/聚合配置应同时包含 `experimental_bearer_token` 与 `requires_openai_auth = true`，官方 API 不得进入 aggregate members。多账号切换前只允许较新的 live 凭据回存，旧 refresh token 不得覆盖账号库；身份或同版本令牌冲突不得静默覆盖。`/v1/images/generations` 与 `/v1/images/edits` 必须使用官方认证且官方失败不得连接任何供应商。

### 2.1 CLIProxyAPI 独立接入

CLIProxyAPI 固定部署到 `D:\pro\CLIProxyAPI`，独立负责账号登录、OAuth 刷新、额度、轮转和账号文件。Codex++ 只管理受管服务进程并调用 `/healthz`、`/v1/models`、`/v1/responses`，禁止读取、转换或同步 CLIProxyAPI 的账号目录。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| Manager 服务控制 | `apps/codex-plus-manager/src-tauri/src/cliproxy.rs` | 固定版本下载与 SHA-256 校验、DPAPI 连接密钥、PID/可执行路径核验、独立进程启停。不得并入 `official_accounts.rs`。 |
| 受管供应商标识 | `crates/codex-plus-core/src/settings.rs` | `RelayProfile.integrationType = "cliproxy"` 用于识别受管通用直连配置；它不成为聚合成员，也不参与账号同步。`cliproxy-official` 只表示第二开关启用的官方模型专用通道。 |
| 独立模型路由 | `crates/codex-plus-core/src/aggregate_model_alias.rs`、`model_catalog.rs`、`relay_rotation.rs`、`relay_config.rs`、`assets/inject/renderer-inject.js` | CLIProxyAPI 模型使用 `CLIProxyAPI:模型名` 直连受管配置，不进入聚合轮转。按钮2开启时官方模型由 `cliproxy-official` 接管，通用通道只展示非官方模型；可信 CLI 官方模型按基础模型继承 Fast 与 reasoning 档位。 |
| Manager 页面 | `apps/codex-plus-manager/src/App.tsx` | 展示状态、API Base URL、连接密钥、模型与测试结果；普通供应商编辑器不得改写或删除受管字段。 |
| 独立配置 | `D:\pro\CLIProxyAPI\config\config.yaml` | 仅首次缺失时生成；已有文件不覆盖。账号维护通过 CLIProxyAPI 的 `/management.html` 完成。 |

**合并确认点**：未安装或未启动 CLIProxyAPI 时原功能不受影响；受管供应商 ID 固定为 `managed-cliproxy`，API Base URL 必须包含 `/v1`。按钮1开启后，CLIProxyAPI 的官方、Gemini 等全部模型都必须进入独立直连组；按钮2关闭时顺序为“官方原生 → 聚合替换项 → CLI 全部模型 → 聚合成员模型”，按钮2开启时顺序为“官方原生 → CLI 官方模型 → 聚合替换项 → CLI 非官方模型 → 聚合成员模型”。CLI 模型必须通过受管配置直连，禁止加入聚合成员、轮转或 failover。`CLIProxyAPI:gpt-5.6-sol/terra/luna` 按各自元数据继承 Fast、默认 reasoning 和 `max/ultra` 等受支持档位；Gemini、普通供应商同名模型及不在可信清单内的模型不得继承。CLIProxyAPI 账号文件变化不得触发 Codex++ 凭据写回、额度刷新或 provider sync。

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
