# gpt-6-luna 起名失败核查

时间均为北京时间，核查于 2026-11-07 00:25 后收敛源码。

## 结论与证据

- 2026-11-06 23:00 后的请求日志中，有 36 次 `gpt-6-luna` 请求以 502 失败，错误均为 `native metadata reader unavailable`，`attempts=0`，未到模型上游。
- 桌面日志 `C:\Users\fgy\AppData\Local\Codex\Logs\2026\11\06\codex-desktop-d6a3b0f5-33f3-49d9-a615-a70f910231de-38740-t0-i1-151436-0.log` 的 1414、4143 行分别在 23:25:35、次日 00:14:46 明确记录 `ThreadMetadataGenerationService: Failed to generate thread title`。这组错误是起名调用，不是定时任务调度失败的证据。
- 运行中的 Codex 为 `26.1103.3517.0`，主窗口前端使用数字命名的 JS bundles，`electronBridge` 冻结；`window.__codexPlusReadNativeThreadMetadata` 缺失。旧读取器只在成功发现旧 ESM broker 后安装。
- 用原生 `mcp-request` 的 `thread/read`、`includeTurns=false` 实测可读同一个本机 app-server，说明是读取器安装依赖失效，不需要放宽身份校验或推测父子来源。
- 保存的 `bindings.codex.titleModel` 是指定官方账号下的 `gpt-5.6-luna`，不是原始请求日志里的 `gpt-6-luna`。本轮没有更改该设置，也没有改账号、认证、供应商或会话 provider。

## 局部修复门禁

- 决策：Local Fix / Local Fix Only，用户已授权核查修复。
- 契约：即使旧模块发现失败或 preload 冻结，也安装只读身份读取器；只允许读取本机 `thread/read` 的必要身份字段，仍校验 UUID、返回身份及元数据完整性，不返回对话正文或认证数据。
- 范围：`70-model-catalog.js`、对应 `native-thread-metadata.test.ts` 和必需重新组装的 `renderer-inject.js`；预算为上述三个文件新增 160 行以内。实际 129 行，不修改 Rust、依赖、全局配置或账号数据。
- 失败、超时只清理监听并返回错误，不重放模型请求、不创建持久后台任务。旧 broker 注入调用继续可用，重复安装保持幂等。
- 基线：原有元数据测试 2 项通过；补充冻结桥回归后先复现 `controlRequest is not a function`，实现修复后通过。
- 停止条件：正式门禁需要处理任务外合并回归或并发发布输入冲突时，不扩大本修复，也不绕过发布脚本复制程序。

## 验证及当前窗口恢复

- 相关 Node 24 测试 229 项通过，包括原生身份字段限制、错误身份拒绝、冻结桥、不同 host 回复、幂等安装、同步/异步失败、超时及监听清理。
- TypeScript `tsc --noEmit -p tsconfig.json` 通过。
- 分片已重新组装且 `--check` 一致；产物 SHA-256 为 `d1e49b97dc0b7fa24530230cb9616703bd140517c91c29a63d0cdf2f0065399c`。任务范围 `git diff --check` 通过。
- 全仓 scope checker 受已有大规模待提交合并与历史未跟踪输出影响未通过，不把任务增量审查冒充全仓小补丁通过；未撤销任何既有改动。
- 已将同一读取函数安装到当前主窗口，实际读回原生根线程身份；没有刷新窗口、重启程序、编辑 config/auth 或重放失败回合。
- 创建一个独立 ephemeral 原生测试线程，确认真实读取器返回 `ephemeral=true`、`modelProvider=custom`，再向本机网关发一次独立起名探针。探针未再出现 metadata reader 错误，但 00:22:53 返回 `response.failed` / 429：所选起名来源的 Key 均在冷却或熔断，提示约 5268 秒后恢复。测试线程已 unsubscribe，没有持久化为用户聊天。
- 此 429 探针是核查行为，不计入原先 36 次 metadata 失败。它说明仍有独立账号额度/冷却阻碍，不能宣称起名端到端成功。未清除冷却、绕过限额或自动切换起名账号。

## 发布交接

- 核查开始时已有发布 `20261106-235459-f7c2c81e` 失败于 core 测试，退出码 101，包含测试断言、trace 及栈溢出等旧合并问题。没有把它报告为仍在正常编译。
- 核查过程中其他执行者启动了 `20261107-002018-8182c6ce`，随后为 `20261107-002330-d6a02fc1`；当前任务没有重复 Start，也没有结束其他执行者的进程。
- `20261107-002330-d6a02fc1/request.json` 已包含本轮产品分片及组装产物的最终哈希，但其测试文件冻结哈希为 `8AA863E59719C36EF3162D9ABBDA1D783369D6F26839C72720E4F4BB09CB14AC`，晚于快照追加的超时/清理测试使当前文件哈希不同。该轮不能直接作为最终交付证据，必须由发布协调者按新输入重建唯一任务，禁止修改冻结清单绕过校验。
- 本文属于未纳入构建快照的说明文档。自此不再修改产品输入，正式交付仍以完整门禁、`state=succeeded` 及最终哈希为准。
- 当前窗口恢复仅属于临时运行态修复；重载窗口或重新启动后，要依靠正式交付的新注入产物安装读取器，不能将临时安装称为已更新磁盘程序。

## 最终交付核对

- 最终任务 `20261107-002847-9d29d5a5` 的冻结输入已包含上述三个文件的最终版本，2026-11-07 01:19:55 完成，`status.json` 为 `state=succeeded`、`stage=complete`，全部步骤退出码为 0。
- 门禁包含 renderer 组装检查、Rust 格式与检查、core/data/manager 测试、TypeScript、前端测试、Vite 及最后的 Rust Release；Release 耗时 23 分 31 秒，没有降低优化配置或另起并行编译。
- 三个 EXE 与四个运行库已交付至仓库 `bin` 及 `D:\pro\Codex++`。备份位于任务目录的 `backup-bin` 和 `D:\pro\Codex++\backups\release-20261107-002847-9d29d5a5`。
- 独立完成 39 次 SHA-256 核对：7 个 release 源文件、14 个交付目标、14 个旧文件备份及 4 个运行库原始来源，全部匹配。两个交付位置的 Manager EXE 均包含本修复独有标记 `native thread metadata read timed out`。
- 交付后未发现残留 cargo/rustc/link 编译进程。`restartPerformed=false`，仍在运行的 Manager 进程均早于此次交付，磁盘文件替换不代表进程已加载新版，需用户自行安排重启。
- 发布证据：`output/release-jobs/20261107-002847-9d29d5a5/status.json`、`summary.md`、`request.json`。本节说明文档更新不改变已冻结的产品构建输入。
- 本地兼容故障已修复并交付；所选起名账号的额度/冷却仍是独立限制，本轮未更改账号或模型设置，也未宣称已完成起名端到端成功验证。
