# CLIProxyAPI 模型复原 Codex 完整 Agent 体验施工计划

日期：2026-09-15

## 当前施工状态

截至 2026-09-15，M0 至 M3 的代码施工已经完成：新增安全的 `codex_app` 工具装配探针，profile 切换会保留内置 MCP/插件配置，只有可信的 `CLIProxyAPI:` 官方模型别名继承官方能力，并在 Responses 请求同时出现新 MCP 与旧 dynamic namespace 时只过滤旧入口。Renderer 探针已兼容 `params.item`、`root.item` 等嵌套工具事件，可从 `namespace = "mcp__codex_app"`、`tool = "read_thread"` 的实际事件中确认 MCP 调用。Manager 已增加 Agent 工具诊断状态，显示 app-server 代际、MCP 工具数量、`read_thread`、最近模型与 provider。

静态检查、Rust 工作区检查、核心/协议/数据/Manager 回归、TypeScript 检查、前端测试和 Release 构建均已完成。2026-09-15 最后一轮验证包括：`cargo fmt --all -- --check` 通过、TypeScript 检查通过、Renderer 定向测试 42/42、前端全量测试 196/196、Release 全工作区编译通过。此前同一施工批次的 Rust 回归结果为 Core 418/418、Protocol Proxy 109/109、Data 103 项、Manager 92/92。

最新前端 bundle 为 `apps/codex-plus-manager/dist/assets/index-Bt5yx_gc.js`，时间为 2026-09-15 22:53:00，已确认包含 Agent 工具诊断文案。新产物已复制到仓库 `bin/`，且与 `target/release` 的 SHA-256 一致：

- `codex-plus-plus-manager.exe`：`067BACB058729213AC3FEDAC32D4298C6EA5B3C6E9089C2DD7CC78323EA32599`
- `codex-plus-plus.exe`：`83579558AB199A5A324AAA7D6DD9772DFC49D4F0C0EA8721574107829FD1A0C8`
- `codex-plus-mobile-relay.exe`：`C291D2BFB8395A60860909C1E54F9C4E1A2D4AEA4EDA24AD2FA1E8A88F1181C9`（本轮无相关代码变化，哈希保持不变）

当前实际运行目录仍是 `D:\pro\Codex++` 的旧副本，因此 M3/M4 的真实桌面验收仍需在完全退出旧 Codex++ 后切换到新产物，并新建 `CLIProxyAPI:` 任务确认探针为 `mcp_ready`、`read_thread` 实际通过 `mcp__codex_app` 成功，同时验证 `exec`、`apply_patch`、Skills、普通 MCP 和审批流程。

## 1. 目标与结论

目标是在 Codex++ 中保留 `CLIProxyAPI:模型名` 作为模型列表来源标识和路由 slug，同时让模型来源只影响推理请求的传输目标，不影响 Codex Desktop/app-server 提供的 Agent 能力，包括任务读取与协作、`exec`、`apply_patch`、Skills、普通 MCP、插件和审批流程。

本次检查的结论是：该目标原则上可行，CLIProxyAPI 中的官方模型可以实现接近官方直连模型的短期替换体验；但不能把“上游模型相同”直接等同于“官方链路完全相同”。完整体验同时依赖三条相互独立的链路：

1. Codex Desktop/app-server 在任务启动时装配宿主工具；
2. Codex++ 将模型 catalog、工具 schema、Responses 请求和流式事件透明传递；
3. CLIProxyAPI 保持所选账号、Responses 状态链和上游协议能力连续。

当前主要故障发生在第 1 条链路，不在 CLIProxyAPI 的模型请求中。Codex 0.154+ 已把任务管理类 App 工具从旧动态工具入口迁移到内置 `codex_app` MCP，但失败任务仍看到了旧 `dynamic_tools.codex_app`，直接 MCP 工具却没有正确暴露，形成“模型看得见旧工具、宿主明确拒绝调用”的断路。

## 2. 已确认的证据链

### 2.1 失败发生在宿主工具层

失败任务：`01a0a2ab-9d9c-7043-8b30-5b3f33296dd1`

对应 rollout：

`C:\Users\fgy\.codex\sessions\2026\09\15\rollout-2026-09-15T09-25-51-01a0a2ab-9d9c-7043-8b30-5b3f33296dd1.jsonl`

任务使用 `CLIProxyAPI:gpt-5.6-sol/luna`，会话元数据包含 `dynamic_tools = [{ name = "codex_app", type = "namespace" }]`。模型调用旧动态入口后，桌面宿主返回：

> This app tool is no longer available through dynamic tools. Use the codex_app MCP server.

该错误在请求到达 CLIProxyAPI 前已经产生，因此不能通过修改 CLIProxyAPI 的模型映射或上游账号解决。

### 2.2 0.153 与 0.154+ 存在明确行为回归

2026-09-04 的成功 rollout：

`C:\Users\fgy\.codex\sessions\2026\09\04\rollout-2026-09-04T11-09-59-01a06a64-fe37-7061-b842-d3951d0c0222.jsonl`

其中：

- app-server/CLI 版本为 `0.153.0`；
- 模型为 `CLIProxyAPI:gpt-5.6-sol`；
- `read_thread` 以 `function_call`、`namespace = "mcp__codex_app"` 发起；
- 宿主记录为 `server = "codex_app"`、`tool = "read_thread"`，调用成功。

2026-09-14 至 2026-09-15 的失败任务使用 `0.154.0-alpha.6.2`，同类模型转而收到旧动态 namespace，并被新版桌面明确拒绝。

### 2.3 内置 MCP 插件存在且已启用

本机插件：

`C:\Users\fgy\.codex\plugins\cache\openai-bundled\codex-app-tools\0.1.3`

插件声明 `codex_app` MCP，并通过 `CODEX_APP_TOOLS_PIPE_PATH` 与桌面宿主通信。它的工具清单不是静态写在 `desktop-mcp.json` 中，而是在任务启动时调用宿主管道的 `tools/list` 动态取得；调用工具时还要求 executor 注入 thread/turn/call 元数据。

当前 `config.toml` 中 `plugins."codex-app-tools@openai-bundled".enabled = true`，未发现用户级 `enabled_tools` 或 `disabled_tools` 排除 `read_thread`。因此“插件未安装”或“白名单手工禁用了 read_thread”不是当前主因。

### 2.4 Codex++ 的模型别名和上游映射本身合理

`CLIProxyAPI:` 目前既是 UI 来源标识，也是 Codex 内部 model slug，进入本地协议代理后再映射为 CLIProxyAPI 接受的真实模型名。该设计可以保留，不需要为了恢复 Agent 功能删除前缀。

关键实现位于：

- `crates/codex-plus-core/src/model_catalog.rs`
- `crates/codex-plus-core/src/model_suffix.rs`
- `crates/codex-plus-core/src/protocol_proxy.rs`
- `crates/codex-plus-core/src/aggregate_model_alias.rs`

当前实际 catalog 中 `CLIProxyAPI:gpt-5.6-*` 已具备 `tool_mode = "code_mode_only"`、`supports_parallel_tool_calls = true` 和 `prefer_websockets = false`。由于 0.153 下同样的 Code Mode 模型曾成功调用 `codex_app` MCP，不能把 `tool_mode` 视为唯一根因。

### 2.5 Responses 转发不是当前首要断点

现有协议代理保留 `previous_response_id`、reasoning 的 `encrypted_content`，Responses 上游响应也基本按原协议返回。当前缺失的是“CLIProxy Responses + 宿主 App MCP 工具”的端到端回归，而不是已有证据表明 Responses 状态字段被删掉。

## 3. 根因分层

### 已证实根因

Codex 0.154+ 不再接受 `codex_app__*` 旧动态工具入口，要求任务通过 `codex_app` MCP 调用；失败任务仍获得旧动态工具目录，而未获得可调用的直接 MCP 目录。

### 高概率原因

扩展模型任务的启动能力装配没有完整继承官方模型任务的 MCP/plugin 环境。模型别名、catalog 元数据、任务启动时的 capability 判定或桌面端兼容注入中的某一处，使 0.154+ 选择了旧动态 namespace，或者在同时存在新旧入口时优先把旧入口交给模型。

### 需要施工阶段探针确认的分支

1. app-server 是否根本没有启动 `codex_app` MCP；
2. MCP 已启动，但 `tools/list` 因任务启动类型或 thread metadata 被过滤；
3. MCP 工具已装配，但发给外接模型的工具 schema 中被旧动态 namespace 覆盖；
4. 旧 catalog 或切换后未重启导致任务继承了过期能力快照；
5. 0.154+ 对自定义 provider/model slug 新增了工具能力门控。

实施时必须先用结构化日志区分以上分支，再选择补丁位置。禁止直接修改官方 `app.asar`，也不在本地伪造一个名为 `codex` 或 `codex_app` 的普通 MCP 来冒充桌面宿主。

## 4. 设计原则

1. **宿主能力与模型路由分离**：`model_provider = "custom"` 只描述本地协议代理传输，不代表禁用桌面 Agent 工具。
2. **保留来源标识**：UI 和内部路由继续使用 `CLIProxyAPI:模型名`，协议代理映射到真实上游模型。
3. **官方模型元数据继承**：当真实模型能精确匹配可信官方目录时，别名条目继承当前官方目录能力；只覆盖 slug、显示名、路由和本地代理必要字段。
4. **非官方模型保守声明**：不因名称像 `gpt-*` 就伪装成官方模型；按真实 Responses、custom tool、reasoning 和流式能力启用功能。
5. **0.154+ MCP 优先**：任务工具只走 `codex_app` MCP。旧动态入口只能用于 0.153 兼容，不得在 0.154+ 暴露为可调用候选。
6. **失败要可解释**：若上游只支持 Chat Completions 或不支持 Codex custom tool，应报告协议能力不足，不得表现为“任务不存在”或无限探测普通 MCP。
7. **配置变更可回滚**：所有 `config.toml` 修改继续使用现有原子写入和时间戳备份机制。

## 5. 分阶段施工

### Phase 0：建立工具装配探针和基线矩阵

目标：在不改变行为的前提下，明确断路发生在哪一层。

工作项：

- 在 Codex++ 诊断中记录安全的结构字段：app-server 版本、任务 model/provider、catalog `tool_mode`、插件启用状态、`codex_app` MCP 是否出现、工具数量、是否含 `read_thread`、是否同时出现旧 `dynamic_tools.codex_app`。
- 对 rollout 增加只读分析器或测试夹具，识别三种调用：`mcp__codex_app`、`server = codex_app`、旧 `namespace = codex_app`。
- 对切换前、切换后、新任务、旧任务分别采样，确认任务能力是否在创建时固化。
- 不记录提示词、工具参数、认证头、API Key、图像数据或任务正文。

建议修改：

- 新增 `crates/codex-plus-core/src/codex_app_tools_compat.rs`，集中放版本判定和安全诊断数据结构；
- `crates/codex-plus-core/src/lib.rs` 导出诊断；
- `apps/codex-plus-manager/src-tauri/src/lib.rs` 暴露只读诊断命令；
- `apps/codex-plus-manager/src/App.tsx` 增加“Agent 工具兼容性”状态行。

完成标准：一次真实任务启动后，可以明确回答“插件未启动、tools/list 被过滤、schema 被旧动态入口覆盖、还是任务快照过期”。

### Phase 1：保证内置 `codex_app` MCP 的安装、启用与配置保留

目标：任何 Codex++ profile 切换都不得删除或关闭官方内置 App 工具插件。

工作项：

- 扩展现有插件市场保留逻辑，识别 `codex-app-tools@openai-bundled` 为宿主关键插件。
- 应用 profile 后校验该插件条目：存在时保留用户显式配置；缺失且桌面版本声明 `installWhenMissing` 时触发正常官方安装/启用流程，不复制私有插件文件。
- 保留 `mcp_servers.codex_app.enabled_tools`、`disabled_tools`、审批模式和超时配置，不用 profile 通用配置覆盖它们。
- 若插件目录、声明文件或启动脚本缺失，Manager 明确提示“官方内置插件缺失/版本不匹配”，不降级为旧动态工具。
- 切换 profile 后若插件/MCP 能力需要新任务或重启生效，界面给出明确提示。

主要修改：

- `crates/codex-plus-core/src/plugin_marketplace.rs`
- `crates/codex-plus-core/src/relay_config.rs`
- `crates/codex-plus-core/tests/relay_config.rs`
- `crates/codex-plus-core/tests/plugin_marketplace.rs`（若现有测试仍内联，可继续放在源文件测试模块）

注意：本机现状已经是插件启用，因此本阶段是必要的防回归基础，但预计不能单独修复当前故障。

### Phase 2：让扩展模型完整继承宿主可识别的模型能力

目标：可信官方模型的 CLIProxy 别名在 Codex 侧拥有与其真实官方模型一致的能力元数据，同时保持独立路由 slug。

工作项：

- 将“显示 slug/路由 slug”和“能力模板 slug”分开建模，例如：
  - display/routing slug：`CLIProxyAPI:gpt-5.6-sol`
  - capability slug：`gpt-5.6-sol`
- capability slug 只能通过当前官方账号目录、bundled catalog 或内置兼容目录的精确匹配产生。
- 克隆官方 entry 后只覆盖允许字段：`slug`、`display_name`、`description`、窗口、优先级、`prefer_websockets = false`、必要的 Responses Lite 设置。
- 不无条件重写模板中的非空 `tool_mode`、custom tool、parallel tool、reasoning、service tier 等能力字段。
- 对历史生成但缺少 `tool_mode` 或关键能力字段的 managed catalog 增加版本标记和自动重建；只重建 Codex++ 自己管理的 catalog。
- 非官方模型继续默认 `code_mode_only`，但通过显式 capability profile 支持 Responses/custom tools，而不是借用官方模型名猜测。

主要修改：

- `crates/codex-plus-core/src/model_suffix.rs`
- `crates/codex-plus-core/src/model_catalog.rs`
- `crates/codex-plus-core/src/aggregate_model_alias.rs`
- `crates/codex-plus-core/src/relay_config.rs`
- `crates/codex-plus-core/tests/model_suffix.rs`
- `crates/codex-plus-core/tests/model_catalog.rs`
- `crates/codex-plus-core/tests/relay_config.rs`

完成标准：同一真实官方模型的裸官方 entry 与 `CLIProxyAPI:` entry 除路由/显示/本地传输字段外，关键 Agent 能力字段逐项一致。

### Phase 3：完成 0.154+ 新 MCP 入口迁移，并隔离旧动态工具

目标：0.154+ 任务只向模型暴露可调用的 `mcp__codex_app` 工具，0.153 保留兼容能力。

先依据 Phase 0 探针选择最小控制点：

#### 分支 A：配置或插件启动缺失

- 通过官方 plugin 配置和任务重建恢复 `codex_app` MCP；
- 不做桌面页面注入层补丁。

#### 分支 B：MCP 已存在，但旧动态 namespace 同时被注入

- 在 Codex++ 可控的任务启动/桌面兼容层中，对 `0.154+` 移除 stale `dynamic_tools.codex_app` 描述；
- 保留直接 MCP server 的工具 schema和 thread/turn metadata；
- 不改其他 App 动态工具命名空间。

#### 分支 C：扩展模型触发 app-server 的错误能力门控

- 优先通过 Phase 2 的 capability template 修复；
- 若 app-server 只按裸 slug/provider 判定且没有公开配置入口，则在 Codex++ 的桌面兼容层做 opt-in 适配，并以 app-server 版本范围保护；
- 适配只改变工具目录装配，不把模型请求改回官方直连，也不伪造模型身份。

兼容规则：

- `<= 0.153.x`：允许既有 `mcp__codex_app` 成功路径；不主动删除仍有效的旧兼容信息。
- `>= 0.154.0-alpha.6.2`：禁止模型选择已被宿主废弃的 `codex_app__*` 动态调用。
- 未知更高版本：先探测直接 MCP；若行为与已知版本不同，关闭补丁并报告版本不受支持，避免长期依赖私有实现。

可能涉及：

- `crates/codex-plus-core/src/cdp.rs` 或对应桌面注入资源，仅在确认控制点位于桌面任务启动层时修改；
- `apps/codex-plus-launcher/src/main.rs`，用于版本探测和启动期诊断；
- `crates/codex-plus-core/src/codex_app_tools_compat.rs`；
- 对应 `cdp_bridge`、launcher 和兼容模块测试。

明确禁止：修改 WindowsApps 中的 `app.asar`、替换官方 `codex-app-tools/server.mjs`、手工设置 `CODEX_APP_TOOLS_PIPE_PATH` 冒充桌面宿主。

### Phase 4：验证 Responses 工具调用透明性

目标：保证工具 schema、tool call、tool output 和最终响应在本地协议代理与 CLIProxyAPI 之间无损往返。

工作项：

- 对 `/v1/responses` 请求建立结构化测试，确认以下字段不被删除或错误转换：
  - `tools` 和 MCP/custom tool schema；
  - `previous_response_id` 或 conversation 状态；
  - reasoning `encrypted_content`；
  - tool call ID、tool output 关联；
  - `response.custom_tool_call_input.*`、完成和失败事件。
- 确认 CLIProxy 官方模型使用 Responses 原生路径，不因 `CLIProxyAPI:` 前缀落入 Chat Completions 转换。
- 上游断流、非 2xx 或缺少终止事件时继续返回合法 `response.failed` SSE，避免 agent loop 死亡。
- 对只支持 Chat Completions 的模型，在选择或首个工具调用前给出明确能力提示。

主要修改：

- `crates/codex-plus-core/src/protocol_proxy.rs`
- `crates/codex-plus-core/tests/protocol_proxy.rs` 或现有协议代理测试模块

### Phase 5：CLIProxyAPI 账号会话粘滞

目标：解决多官方账号轮转导致的上游状态不连续，但不与宿主任务工具故障混为一谈。

工作项：

- 为需要 `previous_response_id`/加密 reasoning 连续性的官方模型启用可配置 session affinity。
- 粘滞键优先使用稳定且不泄露正文的 Codex thread/conversation 标识；没有稳定标识时退化为响应链 ID 哈希。
- 配置 TTL、账号失效后的受控重选和诊断事件。
- 明确 round-robin 与 session affinity 的优先级：同一响应链保持账号，链结束或 TTL 到期后才参与轮转。
- 不把 auth、账号邮箱、token 或完整响应 ID写入普通日志。

该阶段可能同时需要 Codex++ 和 CLIProxyAPI 配置配合，CLIProxyAPI v7.2.103 已提供 `session-affinity` 与 TTL 配置入口。

### Phase 6：Manager 诊断与用户体验

目标：用户能看见“模型可用”与“完整 Agent 能力可用”是两种不同状态。

建议状态项：

- 推理协议：Responses / Chat Completions；
- 宿主任务工具：可用 / 旧动态入口冲突 / 插件缺失 / 需新建任务；
- Skills 与普通 MCP：已装配数量；
- CLIProxy 会话粘滞：开启 / 关闭；
- 当前 app-server 兼容范围。

主要修改：

- `apps/codex-plus-manager/src/App.tsx`
- `apps/codex-plus-manager/src/i18n-en.ts`
- `apps/codex-plus-manager/src-tauri/src/lib.rs`
- 必要时新增独立 Tauri command 模块

## 6. 测试方案

### 6.1 单元测试

- `CLIProxyAPI:gpt-5.6-sol` 精确继承 `gpt-5.6-sol` 的关键能力字段。
- 非官方 `CLIProxyAPI:gpt-*` 不因名称伪装成官方模型。
- managed catalog 缺少关键字段时自动重建，用户自定义 catalog 不被覆盖。
- profile 应用保留 `codex-app-tools@openai-bundled` 及其嵌套配置。
- 0.154+ 兼容层只过滤废弃的 `dynamic_tools.codex_app`，不影响其他动态工具和 MCP。
- Responses 请求/流式事件/tool output 无损往返。

### 6.2 集成测试

使用夹具模拟：

1. 0.153：直接 `mcp__codex_app.read_thread` 成功；
2. 0.154+：新 MCP 与旧动态 namespace 同时出现时只保留新 MCP；
3. 0.154+：MCP 缺失时明确失败，不把请求发送给 CLIProxyAPI；
4. profile 切换前后插件和 MCP 配置保持；
5. 新任务获得新能力目录，旧任务提示需要重建而不是静默失败；
6. CLIProxy Responses 工具回合可完成“模型调用 → 宿主执行 → tool output 回传 → 最终回答”。

### 6.3 真实桌面验收矩阵

| 场景 | 模型路径 | 预期结果 |
| --- | --- | --- |
| 官方基线 | 裸官方模型 | 可读取同一 `codex://threads/...` |
| 官方经 CLIProxy | `CLIProxyAPI:官方模型` | 与官方基线使用同一宿主 `codex_app` MCP |
| 非官方 Responses | `CLIProxyAPI:非官方模型` | 模型支持工具时可读取任务；不支持时明确报能力不足 |
| Chat Completions | 仅 Chat Completions 模型 | 不承诺完整 Agent 替换，明确显示受限 |
| 任务工具集合 | read/list/fork/send/wait/archive/title | 只读工具直接工作；写操作保持官方审批语义 |
| 本地 Agent 工具 | exec/apply_patch/request input | 正常工作，不受模型来源影响 |
| 上下文能力 | Skills、普通 MCP、插件 | 与同一任务配置下官方模型一致 |
| 生命周期 | 新任务、旧任务、恢复任务 | 新任务完整；旧任务有确定迁移/重建提示 |
| 账号路由 | affinity 开/关 | 开启时响应链稳定；关闭时不影响宿主工具 |
| 版本 | 0.153、0.154+ | 两条入口按版本策略工作，无 stale tool |

每个真实验收任务至少记录：app-server 版本、模型 slug、provider、catalog 指纹、MCP 是否含 `read_thread`、实际调用类型和最终状态。不得记录任务正文或凭据。

## 7. 发布与回滚

### 功能开关

新增 opt-in/自动探测开关，例如 `codex_app_mcp_compat`，状态建议为：

- `auto`：默认，按版本和探针启用；
- `off`：完全不做兼容过滤；
- `force_mcp`：仅供诊断，不作为长期默认。

### 回滚粒度

- Phase 1 配置保留可独立保留；
- Phase 2 catalog 能力继承可通过 catalog schema/version 回退；
- Phase 3 桌面兼容补丁必须可单独关闭；
- Phase 5 session affinity 可单独关闭，不影响宿主工具。

### 版本保护

兼容代码必须以 app-server 版本和实际工具目录探针双重判定，不只比较模型名称。检测到未知新版已经原生修复时，应自动停用 Codex++ 的旧动态过滤，避免重复处理。

## 8. 推荐实施顺序与里程碑

1. **M0：诊断闭环**——完成 Phase 0，真实确认当前 0.154+ 属于哪个断路分支。
2. **M1：配置不破坏**——完成 Phase 1，保证 profile 切换不损坏内置插件。
3. **M2：官方别名能力等价**——完成 Phase 2，并重建 managed catalog。
4. **M3：任务工具恢复**——完成 Phase 3，使 `CLIProxyAPI:gpt-5.6-*` 成功读取同一任务。
5. **M4：全工具回合**——完成 Phase 4，覆盖读写任务工具、exec、Skills、MCP。
6. **M5：账号连续性**——完成 Phase 5，再评估“短期完全替换”的稳定时长。
7. **M6：产品化和发布**——完成 Manager 状态、完整回归、前端 dist、Release 构建与哈希验收。

建议不要跳过 M0 直接修改 `tool_mode` 或注入伪 MCP。当前证据已经表明这两种做法无法解释 0.153 成功、0.154 失败的版本分界。

## 9. 最终验收定义

满足以下条件，才可以称为“CLIProxy 官方模型短期完整替换官方模型体验”：

- 同一 Codex Desktop 任务中，仅切换模型为 `CLIProxyAPI:官方模型`，任务工具、exec、Skills、普通 MCP 和插件清单不减少；
- `read_thread` 实际通过 `codex_app` MCP 成功，而不是旧动态入口或模型自行读取本地 rollout；
- 工具写操作保持官方审批流程；
- Responses 多回合、reasoning 状态和 tool output 关联不丢失；
- CLIProxy 多账号环境下，同一响应链不会无控制地跨账号；
- 上游不支持所需协议时明确降级，不伪装成完整体验；
- Codex 版本升级后兼容补丁能自动探测、关闭或明确报警。

达到以上条件后，用户体感可以非常接近官方模型；仍不能承诺永久、绝对 100% 等价，因为 `codex_app` MCP、桌面宿主管道和任务元数据属于官方客户端内部契约，可能随 Codex Desktop 更新变化。
