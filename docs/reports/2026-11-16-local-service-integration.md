# WorkBuddy / CLIProxyAPI / NewAPI 接入体验收敛

2026-11-16 目录归拢补充：当前宿主入口为 `D:/pro/codex-plus-plus`，下文原工作树路径仅为实施时记录；用户拓展源码已移至 `D:/pro/codex-plus-plus/history/plugins/codexpp-local-integrations-20261116`，已安装用户包及外部服务数据未迁移。

## 授权与边界

用户于 2026-11-16 授权统一修改三项接入、完善体验并汇总提交。按 code-quality-workflow 执行 AUDIT → GATE → CHANGE → VERIFY。

产品实现基于实际 2.6.10 工作树 `D:/pro/codexpp-v269-review-20261114`，起点 b8dd3208b；C 盘旧工作树及其并发模型目录改动不动。原有 Cargo.toml、development-workflow.md、LOCAL-vs-REMOTE 报告和 design-lab 变更保留，不并入本次提交。

宿主适配、入口兼容、用户拓展分开验证。新增适配模块超出小修补的行数预算，原因是已批准的跨三项服务接入补齐，而非顺带重写网关。网关继续复用上游 create/update/provider-key、启停与 Agent 配置刷新，不新增选路管道。用户拓展源码现归档于 `D:/pro/codex-plus-plus/history/plugins/codexpp-local-integrations-20261116`；安装只能经 codex_plus_extensions 工具，不修改仓库 bundled 页面源码。

## 已确认问题与处理

| 问题 | 处理 |
| --- | --- |
| 2.6.10 缺 WorkBuddy 宿主路由 | 补回限名、清单授权的 WorkBuddy 接口，仅允许回环地址 |
| CLIProxy / NewAPI 页面接入仍落旧直连配置 | 新增 applyGateway 等 opt-in 操作，旧 apply 及官方通道兼容保留 |
| 重复接入、历史导入、暂停状态 | 按 source / codex:managed-* 复用；更新保持 ID、暂停、价格、命名 Key 及备用 Key |
| 目录读取被当成完整可用验证 | 独立 SSE 测试；HTTP 非成功、失败事件、EOF 缺终止事件均失败 |
| 用户拓展接管后入口与自启动仍绑 bundled key | 入口选择有效用户包；接管/停用后回退；自启动跟随有效服务权限 |
| 远程 NewAPI 等待本机 Docker | 远程和未配置本机 Compose 时不执行 Docker 探测 |
| WorkBuddy 账号目标为空 | 按真实 API 的 file 字段映射，兼容旧 filename |
| 上游断开被展示成在线 | 复用原项目 v2.6.3 account-status.ts 纯判定逻辑 |
| 管理会话只存在内存 | 当前用户 DPAPI 加密持久化，绑定服务地址；401、退出清除，密码不保存 |
| WorkBuddy 仍初始化官方额度、CLI 密钥、iframe 反代 | 新后台只加载 WorkBuddy 管理与既有受管服务控制；旧包文件保留回滚 |

## 验证证据与实际限制

- 新适配四项定向测试通过（含模拟真实 HTTP/SSE、缺终止、失败事件、IPv6、历史来源复用、多 Key/价格保留）。
- 前端入口及桥接八项测试通过；TypeScript 检查通过。
- core 接管权限/关闭后回退定向测试通过。
- Node 管理后台三项测试通过，含真实 Windows DPAPI 往返和地址隔离。
- Playwright 使用相同 manager CSP/沙箱/桥接进行隔离预览；其账号及模型数据明确为模拟数据，不作为真实服务成功证据。
- WorkBuddy 真实既有密钥读取 → 模型目录 → 原生供应商 → SSE 终止事件通过（8.12 秒）；无凭据回显。
- CLIProxyAPI 目录及接入成功，真实生成返回 HTTP 429；不自动换模型、重试或谎报成功。
- NewAPI 宿主尚无模型调用 Key，预检明确拒绝，未写供应商；需用户在页面填写已有调用令牌。
- 完整 WorkBuddy Next.js 页面没有全部移植；当前账号页使用真实 API 和原状态逻辑，更多管理功能由完整管理页入口保留。
- 一次误启动的显式运维测试因缺少服务环境变量被保护性拒绝；随后指定 WorkBuddy 后通过，不属产品回归测试失败。

## 交付与回滚

宿主完整后台发布已成功，发布状态、产物哈希与实际交付证据见下文。现有应用未自动重启，运行中的旧进程尚未加载替换文件。

拓展保留旧文件，升级只切换清单入口及新增文件；停用接管拓展恢复官方本地服务面板，不停止服务或删除账号。管理登录记录在拓展 dataDir 下加密保存，每次替换保留加密备份。网关供应商暂停使用 pauseGateway，不删除旧直连资料。

提交仍需成功交付证据和用户启动新版后的界面确认，遵守 AGENTS.md 验收流程；不自动推送。

## 发布记录与待验收项

- `20261116-013312-4432bef7` 在 rust-fmt 阶段退出 1：既有 reset_credits 模块声明位置不符合格式顺序；只调整声明位置后重新冻结输入。
- 成功任务 `20261116-013416-d1ec027a`：2026-11-16 03:07:26（北京时间）状态 succeeded，十个步骤退出码均为 0；完整 core/data/manager 测试、TypeScript、前端测试、Vite、正式 Release 均通过。前端 2188 通过 / 2 跳过 / 0 失败，Manager 228 通过 / 2 忽略 / 0 失败（真实服务测试需显式执行，不能在普通回归中自动写供应商）。
- 两个用户包已由拓展工具写入，清单无解析错误。写入时热重载被两个既有旧脚本阻止：market-codex-context-used-meter.js、market-codex-list-pagebuster.js；未强制刷新当前对话。最终交付复核 list 均为 loaded、错误为空；新宿主功能仍需重启 Manager 后验收。
- 旧运行宿主不接受 local-cliproxy / local-newapi 的 replaces 值，services 包暂保留空 replaces 以保持有效。新版启动后须通过拓展工具设置接管、确认清单与面板加载，再执行提交。
- 新 WorkBuddy 管理会话需首次登录一次；原密码不保存，成功会话以后由当前用户 DPAPI 恢复。
- 全树范围检查包含既有脏文件（20 文件、879 个已知变化行，另有既有二进制/非 UTF-8 文件），不作为本次独占改动量；仅本次明确文件进入待提交范围。
- 浏览器预览进程与专用测试浏览器已关闭，截图和快照保留；没有停止真实 WorkBuddy、CLIProxyAPI、NewAPI 或 Codex++ 进程。
- 复查补齐尚未接入时更换 WorkBuddy 地址的旧密钥隔离：更换 origin 清除旧迁移 Key 及输入草稿；同 origin 保存保留草稿。真实浏览器在相同 CSP/iframe 桥接下验证两条分支通过，控制台 0 错误/0 警告；仅更新用户拓展，不改变冻结宿主输入。

## 最终交付核对

- 3 个 EXE、4 个既有运行库已交付到本树 bin 及 `D:/pro/Codex++`。7 个编译源产物、7 个封存产物、14 个目标文件及 14 份旧文件备份哈希均独立复核一致。
- Manager SHA-256：`C4C4FB02AD5283B770C0C210607C9CDFDE97BB2F9318E06CAD2D2E0FF6DB93BB`。
- 安装目录备份：`D:/pro/Codex++/backups/release-20261116-013416-d1ec027a`；bin 备份位于该 job 的 backup-bin。没有删除旧版本。
- 证据：`output/release-jobs/20261116-013416-d1ec027a/status.json`、`summary.md`、`agent-verification.json`。Cargo/rustc/link 进程已结束；未自动重启应用。
- 现有六个 Manager 进程均早于新产物生成时间，仍须退出旧管理器并启动已交付版本。拓展清单接管与真实界面验收待这一步完成；当前未提交、未推送。
- 宿主待提交范围为本次 12 个修改文件及新增适配模块、本文，共 14 个文件；独立用户拓展源码共 23 个文件，包含前序历史读取修复补丁。既有非本任务变更继续保留。

## 同日后续修复与当前交付

模型目录超时、桥接循环阻塞及 WorkBuddy 离线启动入口的后续修复见 `2026-11-16-catalog-timeout-workbuddy-start.md`。新发布任务 `20261116-165859-52005205` 已成功，十项门禁退出码均为 0；7 个封存、14 个目标及 14 个备份文件哈希独立核对一致。当前交付的 Manager 哈希为 `E5B5740535982F2737E19DFBF548A1D3C17902B488085431756DDE1A6315C26A`，上述早间哈希只对应前一版产物。

实际运行宿主已接受独立包接管：CLIProxyAPI / NewAPI 1.1.1 loaded 且无错误，local-cliproxy / local-newapi 两个重复内置入口 disabled；已备份 1.1.0 清单。WorkBuddy 0.8.1 同样 loaded，新增概览启动入口；真实启动 operationId=29172 成功，7864/17863 均健康。管理登录、CLIProxyAPI 的实际 429 和 NewAPI 缺调用令牌仍按前述实际限制处理。

未强制刷新 Codex，未重启 Manager，未提交或推送。当前运行进程早于新产物；仍按 AGENTS.md 等待用户启动新版并确认界面行为，再统一提交本任务文件。源码三文件及新报告纳入本次目录修复范围，已有任务外修改不纳入。
