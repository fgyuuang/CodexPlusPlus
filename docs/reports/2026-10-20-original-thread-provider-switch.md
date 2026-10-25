# 原对话来源切换与官方顺序调度修复

日期：2026-10-20。范围：renderer 来源切换边界、Manager 提示及对应契约测试；不修改真实 config/auth、账号库或聊天数据库。

## 结论与根因

- 旧实现进入/退出 `codex_plus_official_schedule` 时必定 fork，其他来源切换失败时也 fork；这是账号副本污染列表的直接原因，本次取消自动副本。
- 旧实现只在 `turn/start` 前切换实例，而模型选择先发送 `thread/settings/update`。本机原生 `0.163.0-alpha.10.2` 的 settings/turn schema 不接受 `modelProvider`，因此仅加字段不会改变实例来源；调度别名可能到达原 `openai` 链路，引发模型不存在/不受支持。
- 本机全局配置已包含调度 provider 和对应模型目录；两个账号缓存均有 `gpt-6.1-sol`，无需通过重写登录或默认 provider 修复。近期日志支持来源未生效这一方向，但未恢复用户每次报错的完整请求，不能将所有模型错误归为同一原因。

## 修改

MCP broker 与 vscode transport 共用原 ID 切换函数，覆盖模型设置、提交消息和活动回合设置入口。同一对话请求串行，缺失扩展路由时强制刷新一次目录。只读探测仍使用 `thread/read` + `includeTurns:false`，来源相同时不 resume。

发生来源变化时，获取 `excludeTurns:true` 的实际运行配置；仅允许 `idle`，随后退订并以原 ID 恢复目标来源。恢复请求保留具名权限或可完整表达的 sandbox、cwd、审批规则、reasoning、协作模式及停用插件。必须验证返回 provider 和 thread ID 后才转发当前请求；输入不进入任何控制请求，不重放、不 fork。

原生 `unsubscribe` 只移除订阅，不能等待它自动卸载实例；真正的 idle 实例替换发生在下一次带来源覆写的 `resume`。已删除验证中发现无效的轮询尝试。

启动登录与推理调度继续分离。进入调度来源后，账号2到账号1的配额调度仍由现有代理处理，不再变更前端 provider；模型错误不能触发额度换号。未修改已有 Rust 调度和认证实现。

## 验证证据

证据目录：`output/schedule-seamless-20261020-212648/`，含任务前字节备份、失败优先回归日志、原生 schema、离线验证及最终测试日志。

- `renderer-final.log`：108/108 通过；覆盖两个传输入口、设置与发送并发、来源一致时不恢复历史、权限/协作状态保留、活动回合/错误状态/来源未生效时不发送、不自动创建副本。分片与组装产物一致，patch version 为 15。
- `native-switch-proof.json`：使用当前安装的原生 Codex，隔离临时 home、虚假凭据及本地 HTTP fixture，无真实账号请求。直接执行产品切换函数：同一 ID 从 `/a` 切到 `/b`，调度别名请求成功，再回 `/a` 成功；`thread/read` 返回新来源，`thread/list` 只有一条，reasoning high 与原审批/sandbox 保留。
- 同一原生验证复现旧 settings 写法：请求仍到 `/a` 并得到 `model_not_found`；新实现遇到该 `systemError` 实例明确拦截，新增上游请求数为零。
- `scope-full.log`：全树 staged/unstaged/untracked 范围检查退出 2，包含已有的大量合并、日志和二进制，不能宣称全仓预算通过。任务相对差异单列在 `scope-task-relative.tsv`；保留任务外变更。

完整 Rust/TS 回归、Vite、Release 和交付核对由 `scripts/release.ps1` 后台任务执行。启动任务不代表通过，只有 `status.json.state=succeeded` 才能称为交付完成。UI 验收与真实账号2耗尽后转账号1的端到端验收尚未完成。

## 原生限制与验收

`0.163.0-alpha.10.2` 只允许无其他订阅者的空闲实例原 ID 重载。运行中/审批中的任务不能中途变更来源；其他窗口持有实例时拒绝发送。已经 `systemError` 的实例不能通过公开接口安全清除该状态，须重启 Codex 后打开原对话，本次没有实现该状态下的完全无缝切换。不会归档、回滚、删除历史或发送额外回合来清除状态。

重新加载来源可能恢复运行时，`excludeTurns:true` 不能保证大历史瞬时完成。后台交付不会重启当前应用；用户启动新版后验收：空闲原聊天选择顺序调度，ID 与列表数量不变；提交后观察 `official_thread_provider_switched` 和实际代理 dispatch；明确额度耗尽才使用备用账号，登录账号保持原样。

旧子代理仍是独立线程，父聊天切换不会迁移它们；现有切换提示保留。不能把本次原 ID 切换验证当作全部历史子代理已迁移的证据。

官方源码参考：[thread_processor.rs](https://github.com/openai/codex/blob/main/codex-rs/app-server/src/request_processors/thread_processor.rs) 的 `resume_running_thread` 和退订处理；[thread_status.rs](https://github.com/openai/codex/blob/main/codex-rs/app-server/src/thread_status.rs)。线上源码可能更新，以上行为同时由本机二进制验证。
