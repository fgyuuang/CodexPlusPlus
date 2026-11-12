# 2026-11-11 网关采用上游选路

## 授权与原因

用户明确要求取消本地网关选路、采用上游方式，并解决 Terra 反复失败。基线是已整合的正式 v2.4.0 `8826008d`，不吸收其后 main。本轮上一任务 `20261111-161341-8be67170` 已成功交付，没有并行发布。

真实 helper 到模拟官方后端的回归，传入不同的 thread/session 和 turn/root-turn、memory_consolidation 元数据。修改前返回 `response.failed`，错误为“Codex 主回合 session 身份冲突”，上游未收到模型请求。原因在本地 `codex_routing::declared_user_root`，不是 Terra 模型名不受支持。

## 实现与兼容

网关 server 恢复上游 `subagent_models::prepare` → `route::resolve_plan` → pipeline 链路。撤下本地 codex_routing 模块及导出；compact 不再查根账号亲和表。pipeline 恢复上游自动组、冷却、配额策略与候选切换，保留来源状态显示及已验证的异步 Box::pin 栈溢出修复。子代理采用上游完整模型身份继承，不再追加本地根账号锁定。

现有官方账号库引用仍作为凭据兼容适配，保留同一刷新所有者，不复制 refresh token，不写 live auth、config 或历史。旧 `local-official-…` 是仍可使用的存量供应商 ID，不是上游模型名；它随显式供应商/模型选路后被替换成真实模型 slug。保留这些 ID 是为了旧配置兼容，不代表本地会话推断仍在执行。新网关登录继续走上游账号登录入口。

图片不再查询旧亲和表；保留现有显式官方目标及不转第三方的限制。多账号自动图片需要用户明确选择官方图片目标，不能由全局当前账号或旧借用标记猜测。

行为变化是用户授权的上游选路采纳：旧“只在明确额度耗尽才切号”和“子代理强制使用根账号”的网关规则退出。上游自动组可能按配额策略、错误分类和用户设置切换候选；不能将其描述为旧本地调度的等价实现。其他直连旧模式、API/Claudian/WorkBuddy 接入和用户插件不在本轮改动范围。

## 验证与交付

门禁与原文件副本保留在 `output/upstream-codex-routing-20261111/`。修复前基线失败；采用上游后，现有 `gateway_e2e` 的真实 helper 全链路通过（1 个顺序场景程序，执行约 13 秒），包括新增裸 Terra 自动组、额度耗尽切换到第二账号、显式旧供应商 ID 的 compact，以及已有其他 Agent 与子代理继承场景。新增三个上游请求的 model 均为 `gpt-6.1-terra`。两次中间重试暴露的是压缩模拟响应缺少合法 checkpoint/item-done；仅修正测试数据，没有放宽产品压缩验证。

产品增量相对本轮备份为 server +8/-95、pipeline +6/-43、mod -1、凭据兼容模块删除 58 行已无调用者的旧策略与断言，另撤下 1,394 行独立本地选路模块；端到端测试新增 109 行。没有增加第二套分派器或策略开关。首次全仓 scope exit 2：包含既有未提交合并和历史输出共 7,590 文件、4,875,836 已知变更行、974 个未知行文件，不能称为默认阶段预算通过；本轮分阶段增量独立审查，`git diff --check` 通过。交付后再次执行全仓默认 scope 仍为 exit 2，完整结果保留在 `output/upstream-codex-routing-20261111/scope-final.log`；发布测试通过不改变这一范围检查结论。

唯一完整发布任务 `20261111-220934-d6298f83` 于 2026-11-11 22:09（北京时间）启动，23:18:56 达到 `state=succeeded`、`stage=complete`，全部步骤退出码为 0。完整 Rust/TS 检查、前端测试和 Vite 均在正式 Release 前完成；Release 优化编译耗时 33 分 55 秒，没有重复启动任务或降低优化设置。

完整回归通过 6,825 项：core 4,318、data 504、Manager 212、前端 1,791，零失败；Rust 忽略 46 项，前端跳过 2 项。core 日志内两个过滤执行的嵌套子进程结果不重复计数。新增 Terra 场景在完整发布中也通过。

三个 EXE 和四个 DLL 已备份并交付到仓库 `bin/` 与 `D:\pro\Codex++`。独立核对 35 项构建、交付和备份哈希均一致，未残留 cargo/rustc/link 编译进程。`config.toml`、`auth.json`、官方账号元数据与加密凭据、用户脚本配置这五个受保护文件的哈希均未变化。Manager 大小 54,024,192 字节，SHA-256 为 `15665E2AC4C42D69572DCC84BCA9BD292B75DF96085925B523FCE132E5B66C2E`；Launcher SHA-256 为 `342D29F5E7D4BE1F2E916049B72E297FAD07A3C5A704A00191F8A6C47626BF98`。

发布证据在 `output/release-jobs/20261111-220934-d6298f83/`，包含 `status.json`、全部步骤日志、`summary.md` 与 `independent-verification.json`。仓库旧文件备份在该任务的 `backup-bin/`，安装目录旧文件备份在 `D:\pro\Codex++\backups\release-20261111-220934-d6298f83/`。

没有自动重启应用。交付核对时 Manager PID 34396、37680、38624 仍分别是 15:01、16:02、16:02 启动的旧进程；文件替换不代表这些进程已加载新版。用户保存工作后完整退出并重开 Codex++ 和 Codex，复核 Terra 裸模型自动组选路与原聊天继续；实际新版运行验收尚未完成，不将模拟官方端到端通过冒充线上会话验收。没有提交或推送。
