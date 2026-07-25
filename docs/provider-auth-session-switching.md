# 官方登录、聚合供应商与会话归属

本文记录 CodexPlusPlus fork 的认证、请求路由和本地会话归属规则。这三类状态必须独立维护，不能再通过同一个 API Key 或 provider 字段互相覆盖。

## 状态边界

| 状态 | 文件/字段 | 责任 |
|---|---|---|
| 官方账号登录 | `~/.codex/auth.json` | 保存 ChatGPT 官方登录态。聚合或独立 API 切换不得覆盖。 |
| 当前请求路由 | `~/.codex/config.toml` 的 `model_provider` | 纯官方默认使用 `openai`；聚合和独立 API 使用虚拟 provider `custom`。 |
| API 供应商密钥 | `[model_providers.custom].experimental_bearer_token` | 仅供第三方或聚合路由请求使用，不写入 `auth.json`。 |
| 会话归属 | rollout `session_meta.payload.model_provider` 与 SQLite `threads.model_provider` | 决定当前入口能否稳定显示本地历史会话。 |

`custom` 是 CodexPlusPlus 聚合路由的虚拟入口，不代表只有一个真实供应商。真实成员通过模型目录、模型别名和聚合 dispatch mapping 选择，因此 Codex 原生 provider 设置只显示 `custom` 是配置结构的结果，不应通过把多个真实供应商写进 `model_provider` 来解决。

## 切换规则

| 活动模式 | 官方登录是否保留 | config provider | 会话同步目标 |
|---|---:|---|---|
| 纯官方登录 | 是 | 默认 `openai` | `openai` |
| 聚合供应商 | 是 | `custom` | `custom` |
| 独立 API | 是 | `custom` | `custom` |
| 官方混入 API | 是 | `custom` | `custom` |

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
