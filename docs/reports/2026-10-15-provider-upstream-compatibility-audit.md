# 2026-10-15：上游供应商更新与本地兼容性专项审计

## 审计范围与当前版本

本轮继续使用 code-quality-workflow 的 diff-regression-scan。只读 fetch 后，上游最新发布仍为 v1.9.0（a5f531d0），origin/main 仍为 e2882d505bd1b97ab3af7b45eb7e0ebd2dd96f5b，与上一轮审计一致。不是又有一个新版本需要覆盖。

本地 Cargo 版本仍为 1.5.0，HEAD 为 90e0004a，MERGE_HEAD 为 f55bb646（10 月 3 日的未完成合并）。该合并点到 origin/main 相差 247 提交、375 文件。本地另有大量 staged/unstaged 扩展，不能把版本号或提交差数当作实际功能差数。本轮没有启动新 merge、rebase、提交或推送。

范围包括供应商配置/保存、聚合别名与路由、官方认证、目录能力、协议转换和既有本地扩展。Claude/语音、多开和新版 UI 作为变更清单核对，不在本轮整包移植。

## 上游最近完成了什么

| 版本 | 主要变化 |
|---|---|
| 1.7.7 | 供应商拆分直连/转换/故障转移/模型聚合；官方 ChatGPT 登录可成为聚合成员；OAuth 刷新与官方模型获取；三种接入方式；供应商身份隔离多开；模型思考等级编辑；启动空白与容量重试修复；移除渠道保护 |
| 1.8.0 | 默认聚合成员裸名、其他成员 cpx 身份名；选择器按供应商折叠；解析失败拒绝静默改派；成员故障隔离；默认成员官方登录同步（默认关）；成员列表与分页保存修复；用量状态栏；自动接管官方 App；搜索压缩兼容、apply_patch 原始输入、目录 schema 修复 |
| 1.9.0 | Claude Code/Cowork 语音输入与增强页重整；SenseVoice 常驻提速；副本检查失败白屏与词条/布局修复；Kimi tool_search 元数据修正 |

来源：[最新发布](https://github.com/BigPizzaV3/CodexPlusPlus/releases/tag/v1.9.0)、[固定版本 CHANGELOG](https://github.com/BigPizzaV3/CodexPlusPlus/blob/e2882d505bd1b97ab3af7b45eb7e0ebd2dd96f5b/CHANGELOG.md)。

## 与本地扩展逐项对照

| 上游项目 | 本地状态与处置 |
|---|---|
| 官方登录加入聚合/失败轮转 | 冲突。本地官方认证源独立，专用账号不参与供应商轮转；保留本地规则 |
| 默认成员发布裸模型并接收任意裸模型 | 冲突。本地官方裸模型必须精确匹配官方目录并直达，供应商使用明确别名；不能替换 classifier |
| 默认成员登录同步到 live auth | 冲突。本地 activeOfficialAccountId 独立管理，不能用聚合默认成员覆盖全局登录 |
| 上游 official_login 刷新实现 | 不替代本地跨进程刷新租约、桌面刷新所有权与网络后 CAS；跨账号隔离优先 |
| 模型聚合 v1 与全局 multi_agent_v2 冲突提示 | 尚未移植；涉及任务历史/加密内容契约，不能只改目录版本号解决账号用量问题 |
| 四分类分页与保存修复 | 本地仍是自己的聚合编辑器和完整配置状态；不能只移植上游的过滤列表保存逻辑。保留 SettingsStore 防空列表/默认列表覆盖保护 |
| 成员临时缺 Key 时被规范化删除 | 本轮已在本地复现并修复；保存使用结构可引用成员，新建/勾选仍校验 URL/Key，已有不可用成员显示保留提示 |
| 按供应商折叠的模型选择器 | 依赖 cpx/default catalog 元数据，本地路由描述符不同，未移植 |
| 官方模型目录与精确身份校验 | 保留本地官方账号专用 provider、历史 provider 定义与关闭式校验；上一轮已修复 renderer 改写来源 |
| Catalog base_instructions 必填 | 上一轮已移植兼容并验证；不重复实现 |
| 会话头透传 | 上一轮已移植三项白名单，客户端认证不透传；出口带账号/thread 关联日志 |
| 纯 API 显式认证/恢复 provider | 本地有等价路径；上一轮恢复路由记忆保存 model/provider 对，保持现有修复 |
| prefer_websockets=false | 本地保留，禁止让本地 HTTP 代理重新走 WebSocket 握手 |
| code_mode_only 默认 | 本地保留；Kimi 能力修正只影响 tool_search，不切换工具模式 |
| Luna 实验 Ultra、Sol Max/Ultra | 本地保留；供应商等级仍依目录校验，不按名字补出 Ultra |
| 插件市场、MCP、provider sync | 本轮不改；上一轮完整回归覆盖，继续保留 openai/custom 自适应目标与第三方市场 |
| 渠道保护移除 | 本地尚保留自己的排队/冷却规则，本轮不随上游批量删除 |

## 发现、证据与处理

### P1 / Must Fix Now：缺凭据成员被保存归一化静默删除

补充审查 App.tsx 后确认：aggregateMemberCandidates 原先按 URL/Key 过滤，normalizeAggregateConfig 又按该候选删成员。一成员暂时清空 URL 或 Key、另一成员仍完整时，normalizeAggregateRelayProfile 在保存前丢掉前者。编辑器更新权重/映射也会使用过滤后的成员列表。上游同类修复适用于本地，已执行独立 gate。

新增测试先得到两项真实断言失败：缺 Key 的 a 被删除；带残留 URL/Key 的纯官方登录仍成为候选。修复将结构引用与当前请求资格区分：缺凭据 API 仍保留引用，纯官方登录/受管独占供应商/嵌套聚合仍排除；新聚合默认成员与新成员勾选仍要求 URL/Key 齐备，已有不可用成员显示“配置未就绪，成员引用已保留”，允许用户手动取消。已知候选为空与缺少设置上下文明确区分，避免官方引用从空候选后门保留，同时保持无设置草稿的旧行为。

四项行为回归使用 TypeScript AST 提取 App 的真实保存归一化函数执行，验证缺 URL/Key 时引用/权重/映射/路由保留、官方/受管/嵌套排除、删除引用与无设置草稿。没有改动运行时成员可用性和失败关闭规则。

发现该问题时已取消第一轮 Release 构建，仅停止本轮 cargo/rustc 子进程；没有发布中间产物，没有停止 Manager/Codex。取消日志独立保留，不当作代码编译失败。
### P2 / Must Fix Now：Kimi 内置能力继承错误

本地九个内置 Kimi 条目未声明 supports_search_tool，从公共模板继承 true。上游 90942234 明确修正该字段；其前置 db1dedb3 记录官方 Responses 接口拒收 tool_search 导致整单 400。

新增测试在修改生产数据前失败：k3 返回 Bool(true)，期望 false。修复仅为九项添加 supports_search_tool=false，其余模型字段与修改前逐项一致，新资产内容与上游同文件完全一致。测试同时覆盖全部九项的 builtin metadata resolver、生成目录、code_mode_only 保留，以及大小写/窗口后缀。

### P1 / Refactor Backlog：Gemini 工具签名回放缺口

本地 chat_tool_call_to_response_item 只保留 id/name/arguments；没有上游 191a9a44 的 extra_content 记录与回放。Gemini 的额外工具签名可能在转换后丢失，严格上游会拒绝后续回合。这里有代码路径证据和上游修复，但未对用户实际渠道发起付费推理验证。

不能直接搬全局缓存：上游早期 ExtraContentCache 以 call_id 为键，没有 provider/session 维度。在本地多账号代理中移植时需要另行验证同名 call_id 的隔离；这属于整合风险推断，不代表已证实本机发生签名串号。最小下一阶段应覆盖 stream/non-stream、历史回放、两来源同 call_id 和缓存淘汰。

### P2 / Refactor Backlog：apply_patch 原始补丁与旧历史

本地 build_custom_tool_call_history 仍将原始补丁解析为操作再重建。上游 542e6c88 改成保留 raw freeform 并迁移旧代理工具历史，涉及源文件约 818 行修改和集成测试迁移。需要同时覆盖新增/删除/移动、多文件 patch、旧 _add_file/_batch 历史、stream 与非 stream；本轮不把整个 protocol_proxy 覆盖过去。

### P2 / Verification Gates：搜索、GLM、压缩的后续兼容

- 上游 5eaa66c9 明确拒绝 Chat 路由的服务端原生搜索，943d8141 验证空搜索参数，67507ca3 兼容 input 别名。本地尚未包含对应校验函数，需 mock 区分客户端工具与服务端工具，不能靠删除所有搜索工具处理。
- 上游 198db931 对特定 GLM 图像输入去掉 data URL 前缀；本地未包含对应模型策略。需按协议/上游验证，不能全局剥离所有图像前缀。
- f2b03d41 的原生 compaction 错误分类与截断流修复尚未整体移植；本地已有 response.failed SSE 与线程存活修复，移植时不能恢复“断流算成功”或对认证/限流盲目摘要回退。
- Kimi db1dedb3/105d8e47 的按实际目标强制禁用、覆盖已保存的 true，尚未移植到所有本地聚合/混合来源路径。当前补丁只修复内置默认值；用户显式元数据覆盖或供应商别名仍需下一阶段路由感知策略。本地混合目录含官方原生模型，不能把整份目录统一置 false，否则会误伤官方工具发现。

## Gate 与改动范围

两个独立的 Local Fix / Local Fix Only 阶段：Kimi 资产及 Rust 测试，预算数据 25 行/测试 35 行；聚合成员保留为 App.tsx 与新增 aggregate-member-retention.test.ts，预算生产 25 行/测试 110 行，前端 dist 是生成产物。字节备份、红/绿测试与 gate 位于 output/provider-compat-audit-20261015。真实 config.toml、auth.json、SQLite、rollout 和用户供应商设置未写入。

原有未完成合并完整保留；需要更大接口迁移的项目列为后续阶段，不以全文件覆盖或临时默认路由绕过本地隔离规则。

## 验证与发布

本轮结果：

| 检查 | 结果 |
|---|---|
| Rust core 全部目标 | 1560 通过，0 失败，7 项既有 ignored |
| Rust data 全部目标 | 195 通过，0 失败，1 项既有 ignored |
| Manager lib（前端更新后重跑） | 101 通过，0 失败 |
| 前端全部测试 | 385 通过，0 失败，1 项既有 skipped；比上一轮新增 4 项成员保留回归 |
| Kimi 专项 / 成员保留专项 | 先红后绿，已计入上述总数 |
| cargo fmt、cargo check、TypeScript | 通过 |
| Vite | 已重建；新提示存在于 dist/assets/index-CB_dHjGJ.js |

上一轮测试、编译与部署证据仍在 output/provider-audit-20261015；本表只列本轮结果。验证汇总 verification-results.json；本轮 task-numstat.json 记录 4 项文件增量（两项生产源/资产及两项测试），报告与生成 dist 单列。Release 已完成，耗时 9 分 12 秒；14:10:08 更新到仓库 bin 与实际目录 D:/pro/Codex++，三处 SHA-256 一致。构建前后源码哈希保持不变，Manager 的更新时间晚于前端 index，二进制中确认包含新 bundle 文件名与完整 Kimi 元数据；上一轮账号修复函数也仍在产物中。

| 文件 | SHA-256 |
|---|---|
| codex-plus-plus-manager.exe | `26BC5E9FB0559FF24216F03DB52F7BED6D8394A9DFB172CCB2232E321EA44285` |
| codex-plus-plus.exe | `8BCFC5F588CC4CF28D987DFB62716094AEBFDDD4D22233972ED4163721FF1765` |
| codex-plus-mobile-relay.exe | `0C22645ADF37A4C65E4462D196A95FEBF5F0347859A391B8A00B75AEEBA3B718`（未涉及代码，内容未变） |

运行目录备份：D:/pro/Codex++/backups/provider-compat-audit-20261015-141006；仓库 bin 备份和完整清单位于 output/provider-compat-audit-20261015/deployment.json。没有重启：Manager PID 27804 仍为 10 月 14 日启动，helper PID 12396 仍为本轮构建前启动。遵循用户稍后自行重启的选择，没有停止应用或聊天。

最终 git diff --check 通过。全工作区 scope checker exit 2，包含既有未完成合并、staged/unstaged/untracked、报告、dist 备份和二进制等共 727 文件，不能报告全树预算通过。每个阶段的任务相对 source/test diff 独立记录，Kimi 为 +9/-5 与测试 +27，成员保留为 App +11/-8 和测试 +72，均在各自 gate 预算内。

## 使用与剩余验证

需从托盘完全退出旧 Manager，再通过 D:/pro/Codex++ 的启动器重新启动，以加载本次嵌入的前端和元数据。Kimi 默认能力在重新生成/应用对应目录后生效；已有显式 true 覆盖和路由能力强制限制未包含在此默认值修复中。

新版本的实际交互验收仍待重启后完成：测试供应商暂清空 URL/Key，再保存聚合其它参数，确认成员引用/权重/映射保留并显示状态提示，填回配置后恢复可用。测试在临时对象与目录执行，本轮未写用户供应商设置，未发起付费供应商推理，也未修改登录凭据、真实 config 或聊天数据库。没有进行整版 1.9.0 合并、提交或推送。本轮后来增加 Manager 前端修复，原先的前端证据复用记录已被新的完整测试与 Vite 构建取代；前端资源与新增提示必须一起进入发布产物。