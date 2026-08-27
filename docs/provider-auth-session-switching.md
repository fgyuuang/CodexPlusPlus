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
- 目标账号先刷新令牌，再停止运行中的 Codex，最后通过现有备份、验证、原子写入与回滚事务切换 live 配置。运行中的 Codex 切换后必须重启。
- 账号可维护名称、分组、标签、顺序和启用状态；活动账号不能直接禁用或删除。
- 导出格式使用 Argon2id 派生密钥与 AES-256-GCM 加密；导入支持该加密包及标准 Codex `auth.json`。凭据不得包含在 Manager 的 settings/accounts 响应中。
- 用量查询使用 ChatGPT Codex usage 接口，手动刷新只强制更新用量缓存，access token 仍仅在接近过期时轮换。Codex 运行期间读取活动账号用量时只使用 live 当前令牌，避免两个写入者争用 refresh token；非活动账号可独立刷新。5 分钟内复用缓存，不做后台高频轮询。

## 混合模式模型路由

官方登录混合 + 聚合供应商使用同一个本地 Responses 入口，但根据请求中的完整模型名进行关闭式分流：

| 请求模型名 | 路由 | 失败行为 |
|---|---|---|
| `gpt-5.6-sol` 等可信官方裸模型 | `https://chatgpt.com/backend-api/codex/responses` | 返回官方错误，绝不进入第三方轮转 |
| `CLIProxyAPI:gpt-5.6-sol` | 按钮2开启时使用 CLIProxyAPI 官方专用配置；关闭时使用通用 CLIProxyAPI 配置 | 只请求 CLIProxyAPI，不进入聚合轮转 |
| `gpt-5.6-sol(供应商1|供应商2:真实模型)` | 对应 aggregate dispatch 成员 | 按聚合策略在该映射成员内轮转 |
| `CLIProxyAPI:gemini-2.5-pro` 等非官方模型 | CLIProxyAPI 通用受管配置 | 只请求 CLIProxyAPI，不进入聚合轮转 |
| `供应商1:gpt-5.6-sol` | 指定供应商 | 仅选择该供应商 |
| `gpt-5.2` 等未知裸模型 | 拒绝 | 不连接任何第三方供应商 |

可信官方裸模型清单固定为：`gpt-5.6-sol`、`gpt-5.6-terra`、`gpt-5.6-luna`、`gpt-5.5`、`gpt-5.4`、`gpt-5.4-mini`、`gpt-5.3-codex`。供应商即使提供同名或其他 `gpt-*` 模型，也只能通过聚合括号别名或 `供应商:模型` 调用，避免与官方同名模型冲突。

CLIProxyAPI 的两个开关职责独立：按钮1控制所有 CLI 模型是否作为受管直连供应商接入；按钮2只控制可信官方 Codex 模型是否使用专用通道并提升到聚合项之前。按钮2关闭时，CLI 官方模型仍由按钮1的通用通道提供，并与 Gemini 等模型一起排在聚合替换项之后。

Codex 模型下拉列表切换时，由 renderer bridge 对 `thread/settings/update` 与 `turn/start` 的 reasoning effort 做目标模型校验。官方裸模型及可信的 `CLIProxyAPI:gpt-5.6-sol/terra/luna` 即使模型目录尚未完成加载，也使用对应基础模型的内置官方能力作为 fallback；其模型描述符同时继承 Fast service tier 与 `max/ultra` 等实际支持档位。能力按具体基础模型判断，Gemini、普通供应商同名模型或未知 CLI 模型不会仅因来自 CLIProxyAPI 而获得官方能力。

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

- 官方直连：`gpt-5.6-sol`
- 聚合替换：`gpt-5.6-sol(供应商1|供应商2:真实模型)`，必须使用 Manager 实际显示的完整半角名称
- 指定供应商：`供应商1:gpt-5.6-sol`

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
