# CodexPlusPlus — 本地功能维护清单

本文件记录本 fork 相对于上游 BigPizzaV3/CodexPlusPlus origin/main 的核心差异功能。
每次上游合并后必须逐条确认回归。以下清单记录需要保留的用户能力及历史实现，不要求永久保留全部本地代码；上游完整覆盖并验证兼容后优先采用上游实现。

## 2026-11-12：v2.6.6 与独立服务拓展

- 正式升级目标为 `v2.6.6`（`949d46b4`），整合源码位于 `D:/pro/codexpp-v266-integration-20261112`、分支 `codex/v266-extension-migration`；原工作区的待提交合并保持原位。构建与交付证据见[本轮报告](reports/2026-11-12-v2.6.6-extension-migration.md)。
- 用户包 `local-services-independent` 通过上游拓展工具安装，名称「本地服务中心（独立）」。后台、页面和测试源码位于仓库外 `D:/pro/codexpp-local-services-extension-20261112`，仅使用通用 Node 后台与页面 API，不调用 fork 的 `/local-integrations/*` 路由。
- 已迁出的边界是现有 CLIProxyAPI 的状态/显式启停、现有 NewAPI Compose 服务的状态/显式启停。只停止拓展自己启动且进程身份仍一致的 CLI；不自动接管旧入口启动的进程，不自动运行服务。NewAPI 不下载镜像、不删卷、不执行 down。CLI 原管理密码只在后台内存按既有 DPAPI 格式解密，不迁移账号目录。
- 供应商创建、模型目录、API Key 管理、自启动及已有 Rust 服务入口尚未全部迁出。官方账号库、历史 provider 恢复和显式官方图片隔离继续保留；不能把此阶段称为已能直接换用纯上游程序。旧服务入口保留至用户实际验收，停用新拓展即可回退，外部数据保持原位。
- 本轮安装时两个旧脚本阻止安全页内热重载，工具未刷新用户聊天；应在新版交付后重启或用户手动热重载，确认拓展状态 loaded，再验收真实启停。mock 测试和只读健康检查不替代真实服务操作验收。

## 2026-11-11：网关采用上游选路

- 按用户明确要求，网关聊天及 compact 改用正式 v2.4.0 的子代理模型继承、`route::resolve_plan` 和候选切换，不再使用本地 `codex_routing` 的会话谱系推断与根账号亲和表。thread/session/root-turn 不同不再被该本地逻辑拒绝。
- 裸模型使用上游自动组；显式供应商/模型、模型组、组固定与配额策略仍在上游框架内处理。旧“只有明确额度耗尽才换号”的网关规则退出，切换条件采用上游分类及用户配置。子代理继承模型身份，自动组不再额外固定到本地记住的根账号。
- 现有官方账号库、加密凭据和旧 provider/model 标识保留兼容，不复制刷新令牌，不批量改写历史，不重新登录。`local-official-…` 作为存量供应商 ID 可以继续存在；它不是独立的模型或选路实现，发送模型仍为真实 slug。
- 官方图片不依赖退役的亲和表，仍要求明确官方来源；图片失败不换账号或第三方。其他直连模式、WorkBuddy/Claudian 与服务插件不改。
- 测试、发布和运行验收见[本轮报告](reports/2026-11-11-upstream-codex-routing.md)。本节覆盖下文涉及旧网关谱系与调度的历史说明。

## 2026-11-10：记忆任务与压缩历史兼容

- 原生明确的独立 memory_consolidation 任务进入既有自动官方组，不要求桌面窗口加载；真正子代理与冲突身份仍确认根来源，不允许未知线程任意换号。
- 单聊天旧 provider 迁移可只读核验 `.jsonl.zst`，仅在历史已是目标来源时修复残留索引，不改压缩原件；通过活动网关 profile 识别官方登录的本机协议代理入口，不将任意 localhost 当网关。
- 定向路由与迁移回归通过；发布及实机验收另记[本轮报告](reports/2026-11-10-memory-task-and-compressed-provider-repair.md)。关联 WireView 原聊天已由另一对话恢复，本轮不干预其执行。

## 2026-11-09：v2.4.0 与历史来源恢复

- 同步正式 v2.4.0 `8826008d`，保留旧合并与本地账号/插件；采用新模型菜单、子回合身份、订阅额度及 Windows 启停改进。
- 新 Codex 的 Rspack 已执行模块缓存补充请求传输发现，旧聊天恢复在原生读取不含模型时读取同 thread 保存模型，再进行精确账号映射；不批量改历史、不使用默认账号替代。
- 普通网关操作继续保留 live 登录，不采用按 account_id 自动清理或借用登录；上游显式登录选择与严格身份/CAS 另行保留。本地官方账号引用仍由原账号库刷新。
- 默认官方裸模型沿用网关自动官方组；独立 app-server 的完整原生用户主回合声明可提供线程身份，不依赖桌面窗口已加载该线程。元数据 UUID 仍纳入谱系确认，子代理不能因缺少单独身份头而绕过根来源检查；旧非 UUID CLI 缓存键保持兼容。
- Terra 两个现存官方账号独立最小请求均已成功，但原调用链的临时线程身份错误仍需最终新版运行验收，不能用探针当作原会话已修复。详见[整合报告](reports/2026-11-09-v2.4.0-provider-recovery.md)。

## 2026-11-06：v2.3.1 与拓展状态核查

- 整合正式 v2.3.1 `7d7832ed`（2026-11-05），不自动吸收其后 main。上一轮未提交的 v2.2.0 合并、插件源码及独立维护工具已做完整快照；未重写用户分支历史。
- 采用上游 MCP 配置解析/备份、26.1029 Fast 和模型菜单、新增子代理模型身份登记、使用模式持久化、拓展包移入废纸篓等修复。官方账号引用、额度耗尽才换号、子代理已确认根来源、图片隔离、旧会话恢复、Claudian/WorkBuddy 及 CLIProxyAPI/NewAPI 插件继续保留。
- 上游失效供应商前缀恢复不得用于显式官方账号：指定账号已关闭/删除、模型不在其目录时必须拒绝，不能映射到默认模型或第三方。子代理仍由已确认根账号决定来源。
- 实机拓展清单核查为 7 个启用并加载、3 个停用，无加载错误；总开关已启用。未复现全部停用，未改用户开关或一键启用。管理页本地列表增加当前宿主、已启用/未启用数量与直接刷新入口，便于区别 Codex/Claude、已安装及实际运行状态。
- 新版代码、检查发布与用户启动验收分别记录。详见[整合报告](reports/2026-11-06-v2.3.1-extension-status.md)。

## 2026-11-05：本地服务插件化

- CLIProxyAPI、NewAPI 管理入口迁为 `bundled:local-cliproxy`、`bundled:local-newapi` 两个可独立停用的管理器插件，在「工具」侧栏与连接页打开。共享界面源码位于 `assets/extensions/local-services/`，发布后每个包包含自足的 `manager.html`；宿主保留现有服务、DPAPI 和供应商适配，不重复实现调用链。
- 插件只使用管理器专用 `/local-integrations/cliproxy`、`/local-integrations/newapi` 能力。每次调用核对总开关、包启用、有效清单、manager.page 和权限；操作限名且参数类型化。返回采用字段白名单，密钥只返回是否存在，不传完整 settings，不扩展 renderer 路由权限。
- 既有安装路径、连接、DPAPI 文件与受管供应商 ID 原地沿用，无账号迁移或自动写配置。普通供应商池接入继续采用上游流程。插件停用阻止 CLI 后续自启动，但不停止现有服务、不删已有供应商；旧页面以「旧版兼容入口」保留至实际验收。
- 官方登录/多账号调度/子代理来源/图片隔离/旧会话恢复仍为核心安全边界；WorkBuddy/Claudian 连接继续用上游 Agent 框架。仓库没有独立 FastAPI 适配；后续此类 API 服务应复用供应商池，管理能力按本插件契约扩展。
- 执行契约和回滚见 [插件化计划](plans/2026-11-05-local-integration-plugins.md)。代码、定向测试与后台发布状态分别记录，尚未以真实服务启停作为验收。

## 2026-11-04：上游优先采用原则

用户允许用新版原生能力替换旧本地实现，核心目标为保留现有登录和历史、多官方账户调度、第三方便捷接入。新增接入优先使用新版供应商池、模型组和通用 Agent 框架；已覆盖的功能不再另建本地实现。

| 能力 | 当前选择 | 仍需核实的边界 |
| --- | --- | --- |
| 第三方 API、供应商池、通用 Agent 接入 | 采用 v2.2.0 上游框架 | 原凭据、模型 ID、历史配置迁移和真实请求 |
| WorkBuddy 使用 Codex++ 模型 | 采用上游 WorkBuddy/WorkBuddy AI 适配 | 模型刷新和实际请求；不等同于 CLI 协作任务执行 |
| Claudian 接入 | 在上游 Agent 框架内增加适配 | 每库独立 CODEX_HOME，断开恢复和插件实际请求 |
| 多官方账号与调度 | 上游池/组框架配合必要本地兼容 | 已有账号只引用，额度耗尽才按序换号，子代理及图片不猜测来源 |
| CLIProxyAPI、NewAPI | 普通 API 接入优先走上游供应商流程 | 旧服务控制、独立账户数据及旧路由尚未证明被完整替代 |
| 旧会话恢复、登录隔离和启动器修复 | 保留尚无等价替代的修复 | 真实恢复及退出验收；不以名称重叠代替行为验证 |

停用旧入口或删除重复代码前须验证替代、迁移与回滚；不自动卸载外部服务或清理账号。当前发布冻结期间仅记录选择，不改变构建源码。详见[本轮整合报告](reports/2026-11-04-v2.2.0-claudian-integration.md)。

## 2026-11-04：v2.2.0、Claudian 与原生 provider 校验

- 目标为正式 v2.2.0 `58ef969c`；保留原分支历史、待提交合并和 11 月 3 日的网关修复，不追入 tag 之后的 main。上游扩展内核、通用 Agent、订阅、新版界面与微信附件流程按正式 tag 合入。
- Claudian 注册为独立 Responses Agent。发现 Obsidian 已登记并配置 Claudian 的笔记库，每库使用 `.claudian/codex-plus` 独立 `CODEX_HOME`，只改 `providerConfigs.codex` 和 Codex 默认模型，主 Codex 登录与其他后端保持原状。接入前存档；断开保留接入期间新增的 CLI 偏好和无关环境变量。WSL 路径不猜测转换。
- 新版原生 Codex 在恢复前校验持久 provider 是否等于当前配置。网关下新建普通分支在创建时使用当前传输及精确来源模型；旧 `codex_plus_official_*` 会话仅在原生确认卸载后，备份并迁移单个 session_meta 与对应 SQLite provider 单元格。消息正文、其他会话、config/auth 不改写；来源不唯一、正在执行或仍加载时停止。实验性预测分支保持原生继承规则。
- 官方账号引用、顺序组仅额度耗尽换号、已确认根来源的子代理、官方图片隔离、官方 HTTP version 契约、供应商短模型显示与启动器防重复激活修复继续保留。网关来源迁移不会把官方账号自动映射到第三方 API。
- 本地 Fast、容量重试、额度空回合恢复、项目移动、Zed/worktree 开关补入新版内置拓展清单。WorkBuddy CLI 第四目标、CLIProxyAPI/NewAPI、聚合映射、插件市场保留和微信持久 inbox/outbox 保留。
- 检查、编译交付与实际启动验收分别记录；详见[本轮报告](reports/2026-11-04-v2.2.0-claudian-integration.md)。

## 2026-11-03：v2.1.4 与原生模型缓存恢复

- 目标为最新正式 tag v2.1.4 `59bdcf1c`；不吸纳 main 中尚未打 tag 的 v2.2.0 及后续开发提交。原 v2.1.1 待提交合并完整备份，隔离 worktree 以 merge 方式继续整合，保留原分支 HEAD 和本地历史。
- Codex 26.1027 的模型查询可能先于注入完成：后端已发布本地来源，原生 React Query 缓存却仍把来源项标成隐藏。补丁只处理 `models/list/local` 缓存，通过已有模型元数据恢复列表；缓存更新后重新处理，重注入移除旧订阅，避免自触发循环。远程主机、其他查询、请求来源和会话配置不变；目录失败时保留原生数据。
- 实机在旧 2.1.1 进程中验证一次内存恢复，原生列表从 9 项变为 40 个可选项，所选官方顺序调度未变。该验证不代表新二进制已交付或重启后的验收。
- 网关接入仍为显式选择，入口为「网关 → Agent 接入 → Codex」，没有自动合并到本地官方调度。当前 Codex 网关绑定关闭；原账号引用和顺序组保留，不自动启用或复制凭据。
- 合入上游 Fast 加载链、启动诊断、Windows 闪窗、网关根级设置保留、限速与连接复用、用量修复和 Agent 并发队列；WorkBuddy 作为第四目标保留，默认并发 1。官方账号顺序不能被窗口排序或本地并发队列改派，仍只允许明确额度耗尽换号，官方图片与子代理隔离规则不变。
- 纯 API 多开只在隔离 CODEX_HOME 中写入已清理的 API 认证，复用配置文件事务的备份与回滚；实例目录指向主目录时提前拒绝。普通来源切换继续保留主登录，官方多开登录行为保留。
- 最终任务 `20261103-012700-ca3f86f0` 已成功：3 EXE、4 DLL 已交付，35 项哈希匹配，主 config/auth 与账号文件哈希不变；新版启动后的界面行为仍待用户验收。
- 回归与交付证据见[本轮报告](reports/2026-11-03-v2.1.4-model-cache-recovery.md)，发布成功与实际启动验收分别记录。

## 2026-10-29：v2.1.1 与旧聊天网关兼容

- 整合目标为正式 v2.1.1 `97278ddf`，不吸纳后续 main 的开发提交。保留本地官方账号库、引用池、顺序组、父子来源、官方图片隔离、聚合映射、CLIProxyAPI/NewAPI、插件缓存与 WorkBuddy；测试采用上游统一 tests/all，保留本地断言并调整路径及全局状态锁。
- 切到网关时，保存过的 `codex_plus_*` provider 定义只在地址确认为相同 ID 的本地 routes 入口时保留，根 provider 和网关目标不变。旧聊天因此仍能加载，按原来源校验和账号库发请求，不重写 SQLite/rollout、不回退第三方。
- 网关隐藏直连菜单，不再连带关闭旧 provider 的请求解析；停用、删除、未知账号和模型仍拒绝。
- 账号引用的模型元数据来自各自账号缓存；缺缓存时仅补可读显示名，不猜测能力。显示名不再退回 `local-official-<hash>/模型`，内部路由 ID 保持。
- 请求状态显示模型与实际选定账号，未选定时明确显示「正在选择账号」；调度换号后更新本请求的来源，不把调度方式当账号名，不串到其他会话，不修改真实请求模型。账号名称按现有规则脱敏。
- 详见[v2.1.1 兼容修复报告](reports/2026-10-29-v2.1.1-gateway-compatibility.md)，运行目录交付和实际界面验收分别记录。

## 2026-10-29：官方用量与账号库引用后续修复

- 官方受保护 HTTP/SSE 处理器补接共享请求日志与按日用量记录，提取上游 usage，不扫描或重复累计历史聊天。完整事件支持跨网络分片；断流、非 2xx 和缺少终止事件保持失败。日志仅保存计数和来源元数据。
- 供应商池可引用现有官方账号库并迁移明确的官方顺序组，保留原登录与调度顺序，不复制 refresh token，不自动切换当前 Agent。只有明确额度耗尽可换号，子代理仍固定根请求已确认来源；普通官方裸模型不允许借此隐式换号。
- 测试覆盖请求日志路径时，用量文件也落在同一隔离目录，并在线程启动前捕获路径，防止测试样本写入真实用户统计。既有疑似样本不自动删除或伪装成历史用量。
- 已获取上游 v2.1.0 及后续 main，当前仅吸纳开发符号瘦身 `5e1d8c1e`，不把产品版本标成完整 v2.1.0。见[后续审计与验证](reports/2026-10-29-official-pool-usage-followup.md)。

## 2026-10-29：v2.0.0 网关与 WorkBuddy 兼容整合

- 目标为正式 v2.0.0（`d2fb3a6e`），使用其新版网关、供应商池、订阅登录、用量和 Agent 管理界面。直连本地账号/聚合/CLIProxyAPI/NewAPI 继续保留，不自动将现有配置迁进网关。
- 网关账号在网关内独立登录；普通启用、重写和断开不借用/创建/覆盖 Codex live 登录。旧借用标记在申请凭据或归还登录时关闭式拒绝，保留双方文件原文并要求独立重新登录，不能按 Team account_id 推断登录归属。
- Codex 内置官方生图只接受唯一或明确选定的 ChatGPT 来源，缺模型同样不能猜测第三方；失败不换官方账号或第三方。其他 Agent 的显式生图配置保留上游行为。
- 新网关已选定的 ForcedRelay 不得被旧直连活动 profile 改派；本地官方身份、父子谱系、临时线程证明、官方裸模型/图片不轮转等规则继续守住。
- 新版共享增强设置目录接入本地 Fast、容量重试、额度空回合恢复、项目移动、Zed/worktree 入口；插件缓存不依赖已废弃 launchMode。采用 Agent 桌宠替代上游已退休 Whale 界面。
- 新网关父子来源根据可信原生谱系及根请求已确认的供应商固定；压缩和子代理图片不另选账号。重启后先由根请求重建绑定，未确认时子代理报错，不能宣称与旧直连的持久来源行为完全等价。
- Agent 协作新增 WorkBuddy 原生 CLI 第四目标，默认停用；旧三目标模型/启用状态/任务记录不变。支持模型、流式结果、精确续聊、取消和超时，不接管桌面聊天或复制任何登录凭据。`available` 仅证明 CLI 文件存在，成功必须有完整终止事件。
- 本机直接 WorkBuddy CLI 已验证可启动，但当前提示 Authentication required，退出码 0 不能当作成功。桌面登录桥复用与真实成功模型任务不在已验收范围内；现有插件保持原状。
- 迁移报告见 [v2.0.0 本地兼容整合](reports/2026-10-29-v2.0.0-local-integration.md)，最终编译交付以本轮 release job 的 succeeded 状态为准。

本轮最终任务 `20261029-123155-f41476b9` 已 succeeded，3 个 EXE 和 4 个 DLL 已交付到 bin 与 `D:\pro\Codex++`，安装目录 Manager 已于 13:39 重新启动。用户进入运行验收并授权提交；不得据此补记尚未完成的 WorkBuddy 成功模型任务。日常修改与缓存策略见[开发说明](development-workflow.md)，完整交付仍保留发布门禁。

## 2026-10-28：v1.12.1 联合迁移

- 目标为正式版 `v1.12.1`（`ca1b212d`），不包含后续 main 的网关开发提交。下文历史记录中的旧可选跟随策略，以本节的新规则为准。
- 官方体验启用时，当前对话全部新旧及嵌套子代理的后续请求跟随根对话当前来源；旧 `followParentAccount=false` 或缺失不再关闭该规则。其他独立聊天保持各自来源。原线程 ID、历史和权限不变，不重放已发请求。
- 标题、描述、环境建议等临时线程没有 SQLite 记录时，仅对已验证的身份缺失通过同一原生 app-server 的固定只读 `thread/read` 校验内存身份及谱系；未知、伪造、跨对话、多父和有环关系仍拒绝。普通持久化 fork 保持独立。
- 协作模式内嵌的模型和推理档位与外层已校验选择同步，避免顺序调度收到旧固定账号别名。
- 官方目录的 bundled、同步版本、异步版本 CLI 探测统一使用 Windows `CREATE_NO_WINDOW`；异步超时结束子进程。原生 MCP 启动与目录探测是不同路径，不删除插件或强杀现有子进程来处理闪窗。
- 保留本地认证隔离、账号目录、括号/供应商别名、CLIProxyAPI/NewAPI、微信 outbox、容量重试、额度空回合恢复与插件市场持久化。新 cpx 聚合显式启用，旧配置不自动转换；官方裸模型和图片不得进入供应商轮转。 官方体验或登录混合模式优先使用本地别名，不发布 cpx 来绕开账号来源；官方账号和独立受管集成不参与 cpx 成员。旧 `syncDefaultMemberLogin` 仅兼容保存，不提供隐式登录同步开关，切换请求目标不能写入或创建官方登录态。
- Windows 保留 Manager、launcher、mobile-relay 原有三个产物路径，launcher 额外提供库供新版 Manager 使用。仍由后台 release.ps1 发布，不自动重启或提交。

## 2026-10-27：账号目录刷新与旧子代理通道

- 手动刷新官方目录覆盖所有启用且就绪的账号；Manager 启动后每 15 分钟在后台刷新并重建模型列表。网络等待后重新读取当前设置再落盘。
- 每个账号的刷新结果必须对应自己的缓存；失败保留该账号快照并报告部分失败，不能显示主账号成功来掩盖次账号刷新失败。
- 官方顺序调度发布账号目录中可见且支持 API 的模型合集，按优先顺序去重；模型能力取自实际提供该模型的账号。分派仍检查目标账号目录，未知模型或无支持账号时拒绝，不能把主账号权限复制给次账号。
- 当时通过旧子代理跟随开关启用；现由上方 2026-10-28 的父子对话统一账号规则取代。
- Codex 26.x 起 `openai` 是保留的内置 provider ID，`model_providers` 里出现该键会让整份 `config.toml` 被拒绝加载（`model_providers contains reserved built-in provider IDs: 'openai'`，桌面端只显示“无法加载 ChatGPT 配置”）。跟随入口因此改由根级 `openai_base_url = http://127.0.0.1:<port>/v1/routes/openai` 表达，内置 provider 的 ChatGPT 登录与 responses 语义不变；历史版本写入的 `[model_providers.openai]` 在刷新配置时整表清除，用户自定义端点（带 `env_key`/`experimental_bearer_token` 或非本地 base_url）保持不动并明确报错。
- 回归覆盖账号目录刷新/失败缓存保留、仅一个账号支持的新模型、未知模型拒绝，以及旧 openai 子线程转到父对话调度来源后实际出站账号凭据。

## 2026-10-25：后续兼容补丁与合并发布

- 按 [更新审计](reports/2026-10-25-upstream-update-review.md) 选择吸纳模型级发送锁和供应商名称修复；不是整版 v1.11.2 合并，版本号保持不变。
- 混入模式对 `additional_rate_limits` 仅解除客户端发送阻断；保留真实用量、重置时间及原始快照，退出混入模式恢复原始状态。服务端额度与账号调度判定不变。
- 编辑供应商地址时保留已有非空显示名称；空名称才回退到 provider 标识，保留本地 `requires_openai_auth` 默认关闭的认证隔离规则。
- 上述补丁与上次尚未交付的桥接修复统一通过后台发布；只有任务状态 `succeeded` 才代表检查及文件交付成功，运行进程仍需用户自行重启验收。

## 2026-10-21：v1.10.1 兼容吸纳边界

已核实上游 tag/main 为 `c541516c`；本地仅选择吸纳兼容补丁，未完成整版合并，版本号不改为 v1.10.1。分阶段计划及验收条件见 [审计报告](reports/2026-10-21-v1.10.1-local-integration-plan.md)。

- 桥接：吸纳 `070685da` 中适用于本地的健康计数和看门狗判定。连续失败至少三次且成功心跳超过 15 秒时，不再被最近失败尝试误判为健康；保留隐藏窗口保护及 5 秒注入宽限，旧 generation 不得污染新状态。
- 注入产物必须从本地分片重组；上游新增的请求状态条模块尚未引入，不能宣称整条状态条修复已移植。
- 请求日志、避震/重试、逐模型参数和会话新架构尚待适配；不得整体覆盖本地代理或设置结构，也不得把一般容量错误当作账号额度耗尽。
- 原有官方认证隔离、同对话切换、顺序调度、旧子代理跟随、官方图片直通、模型别名及插件保留契约继续有效。上游外部 Agent 协作功能不能代替原生旧子代理跟随。
- 耗时检查与交付统一交给后台 `scripts/release.ps1`；下方历史合并命令仅作参考，不能绕过当前 AGENTS.md 的后台发布流程。

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

官方体验启用后，全部新旧和嵌套子代理统一跟随当前根对话来源，旧 `followParentAccount` 值不能关闭该规则。根 `openai` 与 live 登录不变，通过根级 `openai_base_url` 接入本地代理；不得写入保留的 `[model_providers.openai]`。只读原生谱系与内存身份，未知、跨独立对话或无法确认账号模型权限时拒绝。保留线程、历史和权限，不重放在途请求；临时标题任务按同一原生 app-server 的身份校验处理，持久化 fork 保持独立。详见认证状态机文档。

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

**所有带路由前缀的本地 provider 都使用本地传输鉴权。** `ensure_official_experience_config_in_home` 为直连官方账号 2、聚合、CLIProxyAPI 和第三方来源写 `[model_providers.<provider_id>]`，统一设置 `requires_openai_auth = false`、`experimental_bearer_token = NO_AUTH_PROXY_BEARER_TOKEN`。路由 slug 带账号或供应商前缀，若声明 `requires_openai_auth = true`，Codex 会用当前 ChatGPT 登录先校验该 slug，账号 2 的 `codex_plus_official_*:gpt-*` 也会被拒绝为 `The 'X' model is not supported when using Codex with a ChatGPT account.`。直连官方账号 2 的实际凭据由本地代理按已验证的 provider 和模型路由从该账号登录存档读取，转发前将 slug 还原成官方模型名；主官方裸模型仍走 `openai` 原生链路。

**已加载会话换来源必须先卸载再恢复。** 当前 app-server 的 `thread/settings/update` 只更新模型名，`turn/start` 中的 `modelProvider` 不会替换已加载会话的 provider；对已保存的会话，发送前用 `thread/unsubscribe` 卸载，再以目标 `model + modelProvider + reasoningEffort` 调用 `thread/resume`，核对响应的 provider 后才发 `turn/start`。新会话首回合尚无 rollout，必须从 `thread/start` 响应确认 provider；无法确认时关闭式拒绝，不能把账号 2 或第三方模型发给主官方账号。

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
