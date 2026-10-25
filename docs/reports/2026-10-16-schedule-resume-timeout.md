# 顺序调度的历史恢复超时与登录身份分离

## 证据和结论

用户报告“提交消息时出错：账号切换请求超时：thread/resume”，并明确要求使用登录账号启动和加载 Codex 前端，随后独立选择推理调度账号。

2026-10-16 09:27:10，renderer选择 `codex_plus_official_schedule`；09:27:31记录路由失败。只读查询本机 `logs_2.sqlite` 发现，注入层生成的 `thread/resume:0ba7520d-c458-4ed7-b7eb-326d5c7f50ab` 从09:27:11至09:27:42至少持续31秒，包含37条历史记录组装告警。原控制请求固定20秒超时，因此在服务端仍重建历史时拒绝了原发送回合。此次错误发生在请求进入推理调度前，不能归因为账号2额度不足。脱敏统计保存在 `output/schedule-resume-fix-20261016/resume-evidence.json`，没有复制聊天正文或凭据。

原代码的问题是用有生命周期副作用的 `thread/resume` 探测 provider，并在进入调度时先 unsubscribe、再 resume。对于较长聊天，恢复历史和初始化运行环境耗时明显超过轻量查询，且原聊天可能先被卸载。

[官方 app-server 文档](https://learn.chatgpt.com/docs/app-server) 区分了两种操作：`thread/read` 查询存储元数据，不恢复或订阅聊天；`includeTurns=false` 不加载完整回合历史。本次来源探测改用该操作。这里的诊断依据本机日志与实际代码，文档用于确认协议含义。

## 修复契约

- 两条传输路径（MCP broker、vscode请求）均使用 `thread/read`、`includeTurns=false` 查询未知聊天来源。已知且匹配的来源不新增恢复或卸载请求。
- 进入或退出调度 provider 时直接创建保留历史的账号副本，原聊天保持加载。不把 provider 变更伪装成全局登录切换，也不在发送前先 unsubscribe/resume。
- fork沿用目标 model/provider、cwd、权限和推理参数，只省略返回的历史 turns；不复制当前未发送的 input，不自动重发用户消息。用户打开副本后继续。重复提交使用同一已知副本。
- 调度 provider 已确定后，账号2→账号1的选择继续由协议代理负责，不发起聊天恢复请求，不修改全局登录。
- fork控制请求单独允许120秒；只读探测维持20秒上限。超时记录明确的 `provider_control_timeout` 和 `controlMethod`，提示本次消息尚未发送。核心修复是去掉不必要的历史恢复，不能以放宽超时替代此行为修复。
- 固定账号、供应商聚合、模型验证和失败关闭规则维持原有约束。未确认来源或无法建立正确副本时阻止发送，不回落到启动登录账号。

## 启动登录与调度身份

启动登录来自 `activeOfficialAccountId` 及 live auth，负责官方前端登录；根 provider维持 `openai`。`accountScheduleOrder` 只控制明确选用调度来源的推理请求，专用 provider为 `codex_plus_official_schedule`。修改首选/备用顺序不等于切换启动登录。

现场检查时，活动登录与首选恰好均为账号2；这不应成为代码假设。新增/扩展的本地 HTTP mock明确设置“启动登录账号1、首选推理账号2”，检查根 provider仍为openai、调度入口独立、请求认证头属于账号2及其备用账号，且目录生成和调度前后 live auth字节完全不变。

Manager将当前账号标注为“启动登录账号”，说明账号行的“切换”按钮改变登录身份，首选/备用只改变推理调度。旧聊天首次进入调度仍需要打开副本；已有固定来源子代理不会跟随父聊天自动迁移。

## 变更范围和验证

按 `code-quality-workflow` 先审计、记录gate并逐文件备份，新增失败测试后修复。两个新增回归用例在旧实现下失败，模拟 `thread/resume` 永不返回；新实现无需调用它即可完成来源探测或创建副本。另一传输路径覆盖不卸载登录聊天和副本复用。既有固定账号切换测试继续保留。

源码范围为 renderer70、版本00、Manager文案和相应测试；Rust只扩展测试断言，无账号调度策略重写。生成renderer由组装脚本刷新。没有依赖、manifest或锁文件变动，没有处理既有上游合并、提交或推送。真实 config/auth/设置/聊天数据库均未写入，未发起真实推理。

证据、任务前备份、测试日志、构建和部署记录位于 `output/schedule-resume-fix-20261016/`。

| 验证 | 结果 |
| --- | --- |
| core全部测试目标 | 1564通过，0失败，7项原有忽略 |
| data全部测试目标 | 195通过，0失败，1项原有忽略 |
| Manager lib | 101通过，0失败 |
| 前端全部测试 | 392通过，0失败，1项原有跳过 |
| fmt、workspace check、TypeScript | 全部通过 |
| renderer组装一致性、diff whitespace | 通过 |
| Vite | 新文本“启动登录账号与推理调度独立”已进入最新生产包 |

本轮相对任务前备份的源码/测试增量为6文件、+185/-18，生成renderer另列+61/-12；报告与生成dist单列。协议代理Rust文件仅新增34行测试断言，生产调度逻辑没有修改。`relay_config.rs`虽然预先备份，但未修改。

全树scope检查返回2，包含既有未完成合并、staged/unstaged、untracked、历史备份及二进制；不能宣称整个工作区符合阶段预算。任务增量见 `task-numstat.json`，全树日志见 `scope-full.log`。既有工作区内容全部保留。

构建前冻结的407项输入SHA-256在编译前后完全一致。前端资源于09:40:42生成；Release构建于10:15:01完成，耗时10分23秒，Manager产物晚于前端资源。于10:15:22部署到 bin/ 与 D:\pro\Codex++，部署脚本逐个确认三处哈希一致：

| 产物 | SHA-256 |
| --- | --- |
| codex-plus-plus-manager.exe | E72E2AD17A503601523B1A91C518C3F5FCDBDC26CC9928D423E8CB9F5444A311 |
| codex-plus-plus.exe | 9B267BAAC6012141A521238A4B8C75BC53AD2CF00504CEF3C1F73CEEB6A95C68 |
| codex-plus-mobile-relay.exe | 0C22645ADF37A4C65E4462D196A95FEBF5F0347859A391B8A00B75AEEBA3B718 |

运行目录旧文件备份在 D:\pro\Codex++\backups\schedule-resume-fix-20261016-101519，仓库 bin/ 旧文件备份在本轮证据目录。部署时 Manager PID 36320（10月15日启动）与启动器 PID 11224（10:00启动）仍在运行，所以文件已经更新，但这些进程仍加载旧代码；部署记录为 restartPerformed=false。复核日志和产物不会自动重启它们。

## 使用边界

交付新文件后需要完全退出旧 Manager/启动器并加载新版注入；不会自动结束用户正在运行的聊天或子代理。此前超时的原消息没有被本次修复自动发送。已有聊天首次进入调度时打开提示中的账号副本；直接新建并选择“官方顺序调度”来源也可避免旧来源转换。

本地模拟验证不能替代真实服务端历史兼容和额度扣减验收。此次日志证明历史恢复超时，不证明每一次历史路由失败都具有同一根因。
