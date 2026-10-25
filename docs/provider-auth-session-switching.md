# 官方登录、聚合供应商与会话归属

本文记录 CodexPlusPlus fork 的认证、请求路由和本地会话归属规则。这三类状态必须独立维护，不能再通过同一个 API Key 或 provider 字段互相覆盖。

## 状态边界

| 状态 | 文件/字段 | 责任 |
|---|---|---|
| 官方账号库 | `official-accounts.json`、`official-account-secrets.json` | 元数据与凭据分离；Windows 凭据使用当前用户 DPAPI 保护，非 Windows 文件限制为当前用户可读。 |
| 活动官方账号 | `activeOfficialAccountId`、`~/.codex/auth.json` | 账号库保存多个登录，活动账号的凭据原子写入 Codex live auth。 |
| 当前请求路由 | `~/.codex/config.toml` 的 `model_provider` | 纯官方默认使用 `openai`；聚合和独立 API 使用虚拟 provider `custom`。 |
| API 供应商密钥 | `[model_providers.custom].experimental_bearer_token` | 仅供第三方或聚合路由请求使用，不写入 `auth.json`。 |
| 官方登录混合模式 | `officialLoginMixedMode`、`activeOfficialAccountId` | 活动官方账号作为认证源；第三方或聚合 profile 仍作为请求目标。`officialLoginRelayId` 仅用于旧配置迁移兼容。 |
| 会话归属 | rollout `session_meta.payload.model_provider` 与 SQLite `threads.model_provider` | 决定当前入口能否稳定显示本地历史会话。 |

`custom` 是 CodexPlusPlus 聚合路由的虚拟入口，不代表只有一个真实供应商。真实成员通过模型目录、模型别名和聚合 dispatch mapping 选择，因此 Codex 原生 provider 设置只显示 `custom` 是配置结构的结果，不应通过把多个真实供应商写进 `model_provider` 来解决。

在官方登录混合模式中，`custom` 同时也是本地协议代理的传输标识。官方裸模型和官方图像工具仍可由代理使用 ChatGPT 登录直连 OpenAI；因此“会话 provider 为 custom、模型为官方裸模型”并不等于请求泄露给第三方。不能仅为显示统一而把这些会话元数据强制改成 `openai`：Codex 恢复会话时可能据此绕开本地代理，使聚合别名和指定供应商模型失效。Manager 应把该状态解释为“官方混合代理”，底层仍保留 `custom`。

## 切换规则

| 活动模式 | 官方登录是否保留 | config provider | 会话同步目标 |
|---|---:|---|---|
| 纯官方登录 | 是 | 默认 `openai` | `openai` |
| 聚合供应商 | 是 | `custom` | `custom` |
| 独立 API | 是 | `custom` | `custom` |
| 官方混入 API | 是 | `custom` | `custom` |
| 官方登录混合 + 独立 API | 先恢复所选官方账号 | `custom`，`requires_openai_auth = true` | `custom` |
| 官方登录混合 + 聚合供应商 | 先恢复所选官方账号 | `custom`，`requires_openai_auth = true` | `custom` |

官方登录混合模式的执行顺序固定为：

1. 从 `activeOfficialAccountId` 指向的加密账号库恢复 ChatGPT 登录态；旧版官方 profile 的有效 `authContents` 首次加载时迁入账号库。
2. 写入当前选中的独立 API 或聚合供应商配置，由 `experimental_bearer_token` 覆盖实际请求认证。
3. 官方 API 不写入 `AggregateRelayProfile.members`，也不参与失败切换、轮转或权重计算。
4. 官方原生模型保持原名；聚合替换项使用半角字符，例如 `gpt-5.4(供应商1|供应商2:真实模型)`。

## 多官方账号生命周期

- 支持隔离浏览器 PKCE OAuth 和设备码登录；稳定身份由 JWT subject、ChatGPT account id 与 workspace id 组合生成，重复登录更新已有账号。
- 切换前先校验当前 live `auth.json`；仅当 live access token 过期时间或 `last_refresh` 更新时才回存。账号库凭据更新时忽略旧 live 文件，同版本却包含不同 refresh token 时关闭式拒绝，只有用户明确确认后才能放弃冲突文件。
- 目标账号先校验令牌，再停止运行中的 Codex，并回存停止期间的最终凭据。切换使用的目标凭据在模型目录刷新后读取，避免目录的 401 重试轮换令牌后又写回旧快照；最后通过现有备份、验证、原子写入与回滚事务切换 live 配置。运行中的 Codex 切换后必须重启。
- 账号可维护名称、分组、标签、顺序和启用状态；活动账号不能直接禁用或删除。
- 导出格式使用 Argon2id 派生密钥与 AES-256-GCM 加密；导入支持该加密包及标准 Codex `auth.json`。凭据不得包含在 Manager 的 settings/accounts 响应中。
- 用量查询使用 ChatGPT Codex usage 接口，手动刷新只强制更新用量缓存，access token 仍仅在接近过期时轮换。Codex 运行期间读取活动账号用量时只使用 live 当前令牌，避免两个写入者争用 refresh token；非活动账号可独立刷新。5 分钟内复用缓存，不做后台高频轮询。
- 旧 profile 迁移、live 凭据回存的版本比较与写入在同一次账号库加锁中完成；相同、较旧或冲突的凭据不覆盖账号库，也不把 `needsReauth` 重置为 `ready`。
- Manager、模型目录与协议代理按账号使用跨进程刷新锁；等待期间已有有效的新令牌时直接复用。网络请求完成后再次核对原凭据，避免覆盖期间完成的重新登录。桌面端运行时不得轮换它持有的当前登录；仍有效的临期 access token 可以继续使用，401 或实际过期时由桌面端处理。桌面端未运行时，当前登录刷新成功后先备份再原子同步 live `auth.json`。网络错误不等同于需要重新登录。
- 官方体验模式的历史会话可能引用 `codex_plus_official_<账号 ID>`。账号变为主账号后，菜单用 `openai` 展示其原生模型，但配置必须继续保留历史账号 provider 定义；不能依据当前菜单目录删除这些定义。历史请求仍按原账号 ID 和该账号可信模型目录解析，禁止改用当前活动账号或进入供应商轮转。账号禁用、删除或需要重新登录时，定义保留用于加载历史会话，请求则关闭式拒绝。

## 混合模式模型路由

### 子代理与切换来源

Codex 子代理创建时继承父回合的 provider；账号专用 provider 必须保留到子请求，即使子代理使用官方裸模型名。Renderer 的恢复记忆必须同时保存 model/provider，不能把账号专用 provider + 裸模型改写为 `openai`，也不能在后续无模型的回合中丢失账号来源。明确选择主官方来源时仍允许切到 `openai`。

已经创建的子代理是独立线程，原生 provider 不随父聊天切换。默认关闭跟随时，向旧子代理继续发任务仍使用其自身来源；可停止后重新创建，或启用下面的官方账号跟随。`openai` 是启动登录入口，不是固定账号 ID 的别名。不能声称父聊天切换成功意味着已经迁移全部后台代理。

Manager 的“旧子代理跟随父对话账号”（`officialExperience.followParentAccount`）默认关闭，启用/关闭后须重启 Codex。启用期间，根 `model_provider` 和 `auth.json` 保持不变；仅将官方 `openai` 的 Responses 传输接到本地受管代理，使此前加载的官方子线程也能在下一次启动后接受路由校验。不得覆盖用户自定义的 openai 端点或认证。关闭后移除这段受管 base URL，恢复原生官方传输。

每次官方请求均只读原生 `thread_spawn_edges` 和 `threads`，沿父子关系找到根对话当前 provider；只对真实子线程应用跟随，根对话和其他聊天保持来源。子线程 ID、模型、推理等级、工具状态和历史不改写，旧 provider 别名只在验证原来源后还原为基础模型，再用目标账号自己的目录和凭据校验。关系缺失/有环/多父/标识跨对话、账号不可用或模型不存在时停止请求，不回落到账号1。第三方子代理不参与官方账号跟随。请求已发出时不终止、不重放，下次请求才采用新来源。

跟随覆盖 Responses、compact 和固定官方账号的内置 `gpt-image-2` generation/edit。调度来源的图片请求缺少固定账号绑定时明确拒绝，不推测账号或轮转。子代理的历史 provider 字段仍可显示旧来源，实际账号以 `protocol_proxy.direct_official_dispatch` 和图片的 `protocol_proxy.child_official_image_dispatch` 为准；`protocol_proxy.child_account_follow` 记录已验证的来源变化，不代表请求已完成。

### 原对话内切换与顺序调度

切换模型来源不再自动调用 `thread/fork`，失败时也不创建账号副本。两个 renderer 传输入口都在模型设置或提交消息前确认真实来源；同一对话的设置与提交按顺序处理。`thread/read` + `includeTurns:false` 只负责轻量查询，来源一致时不重载。

来源变化时，先用 `thread/resume` + `excludeTurns:true` 取得实际运行配置，在空闲边界退订，再以同一个 thread ID 恢复目标 provider，并保留权限、cwd、推理档位、协作模式和停用插件。只有返回的真实 provider 与原 thread ID 符合要求时才发送当前请求。原生 `thread/settings/update`、`turn/settings/update` 和 `turn/start` 不接受 provider 覆写，不能只附加字段后直接发送路由别名。

当前安装的原生 Codex `0.163.0-alpha.10.2` 只允许无其他订阅者的 `idle` 实例这样重载。活动回合、审批中的任务或其他窗口持有的实例必须拒绝切换。`systemError` 实例也不能重载来源：须重启 Codex 后打开原对话，无需新建聊天；不得通过归档、删除、回滚历史或发送空耗额度的回合来规避限制。`excludeTurns:true` 减少返回历史，不代表运行实例恢复无需时间。

进入 `codex_plus_official_schedule` 后，账号2到账号1的顺序选择由本地代理完成，不再切换前端 provider、恢复聊天或改动启动登录。仅明确额度耗尽触发后备账号，模型不存在、认证失败或普通限流不作为换号依据。旧子代理仍遵守上一节的独立线程边界。

本地 Responses 代理仅透传 `session-id`、`thread-id`、`x-opencode-session` 三个会话头，不透传客户端认证、Cookie 或任意其他请求头。默认不根据会话头改派账号；只有显式开启跟随并经本地父子关系验证后，才允许子线程采用父来源。账号专用请求在验证账号凭据身份且收到响应头后记录 `protocol_proxy.direct_official_dispatch`，字段包含本地账号 ID、provider、上游模型、操作、UUID 会话关联和状态码，`phase=response_headers` 不表示流已完成。关闭跟随时原生 `openai` 直连不经过代理，不能凭缺少代理日志断言没有用量。

官方账号专用入口 `codex_plus_official_<账号 ID>` 的 Responses 请求允许两种模型表示：菜单使用的 `<provider>:<模型>` 路由别名，以及 Codex 原生请求/历史会话使用的裸模型名。两者均绑定 URL 中准确匹配的启用且 `ready` 的账号；裸模型必须精确存在于该账号已缓存的官方目录，缺少该目录、模型未知或别名属于其他账号时关闭式拒绝。不能借用当前主账号目录、当前 live 登录或供应商轮转来替代这个账号。

官方体验的次账号菜单使用该账号已缓存的官方目录，不能与当前账号目录取交集。只有启用且认证状态为 `ready` 的次账号参与展示。官方目录或用量请求实际成功后，可将过时的 `needsReauth` 恢复为 `ready`；恢复时必须在账号库锁内核对请求凭据及请求前的账号快照，防止覆盖并发重新登录或状态更新。缓存命中、令牌本地有效、旧凭据迁移不构成认证成功证据。

官方登录混合 + 聚合供应商使用同一个本地 Responses 入口，但根据请求中的完整模型名进行关闭式分流：

| 请求模型名 | 路由 | 失败行为 |
|---|---|---|
| 当前官方目录中的可信裸模型 | `https://chatgpt.com/backend-api/codex/responses` | 返回官方错误，绝不进入第三方轮转 |
| `CLIProxyAPI:<官方目录中的 slug>` | 按钮2开启时使用 CLIProxyAPI 官方专用配置；关闭时使用通用 CLIProxyAPI 配置 | 只请求 CLIProxyAPI，不进入聚合轮转 |
| `<官方 slug>(供应商1|供应商2:真实模型)` | 对应 aggregate dispatch 成员 | 按聚合策略在该映射成员内轮转 |
| `CLIProxyAPI:gemini-2.5-pro` 等非官方模型 | CLIProxyAPI 通用受管配置 | 只请求 CLIProxyAPI，不进入聚合轮转 |
| `供应商1:gpt-5.6-sol` | 指定供应商 | 仅选择该供应商 |
| `gpt-5.2` 等未知裸模型 | 拒绝 | 不连接任何第三方供应商 |

可信官方裸模型不再使用固定清单：只有当前活动账号目录、Codex bundled catalog 或仓库兼容目录中精确存在的 slug 才能作为官方裸模型。供应商即使提供同名或其他 `gpt-*` 模型，也只能通过聚合括号别名或 `供应商:模型` 调用，避免与官方同名模型冲突。

CLIProxyAPI 的两个开关职责独立：按钮1控制所有 CLI 模型是否作为受管直连供应商接入；按钮2只控制可信官方 Codex 模型是否使用专用通道并提升到聚合项之前。按钮2关闭时，CLI 官方模型仍由按钮1的通用通道提供，并与 Gemini 等模型一起排在聚合替换项之后。按钮2开启后，官方目录刷新与 CLIProxyAPI 模型刷新都会重新计算专用 profile，并在模型集合变化时原地更新当前混合聚合 catalog；该同步只写设置和 catalog，不负责启动 CLIProxyAPI，也不关闭或重启 Codex。

Codex 模型下拉列表切换时，由 renderer bridge 对 `thread/settings/update` 与 `turn/start` 的 reasoning effort 做目标模型校验。官方裸模型及 `CLIProxyAPI:<官方 slug>` 的能力只从官方目录条目读取；目录尚未完成加载或条目不存在时不猜测 reasoning/Fast 能力。能力按具体基础模型判断，Gemini、普通供应商同名模型或未知 CLI 模型不会仅因来自 CLIProxyAPI 而获得官方能力。

供应商或聚合 Responses 流出现超时、非 2xx、传输中断或未发送终止事件时，本地协议代理统一以 HTTP 200 SSE 返回 `response.failed`，并记为 `helper.protocol_proxy_stream_failed`。该失败只结束当前 turn，不得破坏官方认证、会话 provider 或后续切回官方模型的 `thread/settings/update`。

CodexPlusPlus 管理的 model catalog 对本地 HTTP 代理统一写入 `prefer_websockets = false`。因此官方模型在混合模式下使用 HTTP Responses 转发，不会先对 `127.0.0.1:57321` 发起无法完成的 WebSocket 握手；官方 HTTP 错误也不会被记录成聚合成员失败。

## 官方图像工具路由

Codex 内置 `image_gen` 固定使用 `gpt-image-2`，并通过活动模型 provider 请求 `images/generations` 或 `images/edits`。混合模式的活动 provider 基址是本地代理，因此 CodexPlusPlus 必须接管以下入口：

```text
POST http://127.0.0.1:57321/v1/images/generations
POST http://127.0.0.1:57321/v1/images/edits
```

代理使用所选官方账号的 ChatGPT access token 与 account id，分别直通：

```text
https://chatgpt.com/backend-api/codex/images/generations
https://chatgpt.com/backend-api/codex/images/edits
```

该路径只在官方登录混合模式启用，不读取 `experimental_bearer_token`，不选择聚合成员，不做第三方 failover。官方返回的状态码、Content-Type 与 JSON 图像结果原样返回；诊断日志只记录操作、端点和状态，不记录提示词、请求正文、图像数据或认证头。此路径不依赖 `aiapi.cc.cd`，也不要求 Platform `OPENAI_API_KEY`。

## Claudian / 外部插件调用

外部插件统一调用：

```text
POST http://127.0.0.1:57321/v1/responses
Content-Type: application/json
```

请求体中的确切模型名分别为：

- 官方直连：`<官方目录中的 slug>`
- 聚合替换：`<官方 slug>(供应商1|供应商2:真实模型)`，必须使用 Manager 实际显示的完整半角名称
- 指定供应商：`供应商1:<官方或供应商模型>`

`POST /v1/chat/completions` 仍只用于活动供应商声明为 Chat Completions 的协议转换，不提供官方 ChatGPT 裸模型直连。需要官方混合路由的 Claudian 配置必须使用 `/v1/responses`。

启动前自动同步由 `provider_sync_target_for_settings()` 决定目标。Manager 手动同步允许选择 `openai`、`custom` 或扫描发现的历史 provider，并显示：

- 唯一会话数：rollout 与 SQLite thread id 的并集；
- rollout 数：该 provider 对应的唯一 rollout thread 数；
- SQLite 数：所有候选会话数据库中该 provider 对应的唯一 thread 数。

如果 rollout 和 SQLite 的 provider 统计不一致，说明客户端曾按另一配置重新索引。必须先切换到目标供应商配置，再执行同步；否则运行中的 Codex 可能再次按当前 `config.toml` 覆盖 SQLite provider。

## 限制

- provider sync 只修改本地元数据和索引，不会把本地对话上传到 OpenAI 云端。
- 同一个 thread id 不能通过复制方式同时成为两个独立会话；目标切换采用归一而不是复制。
- 历史 `encrypted_content` 可能绑定原 provider 或账号。元数据同步后会话可以显示，但继续对话或压缩上下文仍可能出现 `invalid_encrypted_content`。
- 同步写入前必须备份 rollout 和 SQLite；被占用的 rollout 必须跳过并在结果中列出。

## 上游合并人工确认

上游更新涉及以下任一位置时不得只依赖自动冲突解决：

- `auth.json`、ChatGPT 登录或账号状态；
- `config.toml` 的 `model_provider`、`requires_openai_auth`、bearer token；
- Codex 启动顺序或供应商切换；
- rollout `session_meta`；
- SQLite `threads` schema、数据库路径或重建逻辑；
- 模型 catalog、聚合 alias 或 dispatch mapping。

合并后至少验证：官方登录切换到聚合时 `auth.json` 不变；聚合启动后会话归一到 `custom`；纯官方启动后会话归一到 `openai`；Manager 能同时显示 provider 统计并手动选择同步目标。
