# 官方账号刷新与凭据同步修复

日期：2026-10-07。复核来源为 WorkBuddy 对话「排查 codex++ 账号刷新失败与启动缓慢」。

## 已完成的改动

- `official_accounts.rs` 增加原子凭据合并：版本比较与保存共享同一次账号库锁，旧 profile 迁移、live 迁移、Manager 回存及桥接刷新统一调用。相同、旧版或冲突的凭据不覆盖账号库。
- 同一账号的令牌轮换使用跨进程文件锁。等待期间若别人已提供有效的新令牌，复用新版本；网络请求完成后比较原凭据，避免覆盖期间完成的重新登录。
- 官方模型目录、用量查询和官方协议代理通过共同的 `refresh_tokens` 入口受到保护。运行中的桌面端持有当前登录时，Codex++ 不轮换它的 refresh token；临近过期但仍有效的 access token 可继续使用。401 或实际过期会返回明确错误，等待桌面端处理。
- 桌面端未运行时，当前登录轮换成功后备份并原子同步 live `auth.json`。网络连接错误不再直接把账号标记为 `needsReauth`；HTTP 400/401 的状态更新也会检查凭据是否已经被重新登录替换。
- 修复切换账号的旧快照顺序：桌面端停止后再次保存最终 live 凭据，目标登录快照在模型目录刷新之后读取，避免目录的 401 重试轮换令牌后仍写入旧快照。
- 更新认证与会话状态机文档。第三方路由、provider 归属和图像认证隔离规则保持原有契约。

变更范围是官方凭据生命周期；执行方式为分阶段局部修复。没有新增依赖、修改用户 config.toml、人工覆盖登录凭据、提交 Git 或推送远程。原有源码备份与检查日志保留在 `output/official-auth-fix-20261007-105455/`。

## 验证结果

| 检查 | 结果 |
| --- | --- |
| 官方账号相关单元测试 | 21 通过，包含 10 个本次新增的账号回归测试 |
| bridge_routes / protocol_proxy / relay_config / relay_switch 集成测试 | 32 / 150 / 185 / 24 通过，共 391 项 |
| Manager 最终库测试 | 101 通过，含目录刷新后读取切换凭据的新测试 |
| 数据层测试 | 195 通过，1 跳过 |
| 前端测试，使用本机 Node 24.19.0 | 379 通过，1 跳过 |
| TypeScript 检查 | 通过 |
| cargo fmt / 改动文件的 diff 空白检查 | 通过 |
| 最终 cargo check --workspace | 通过，有现有警告 |
| Vite 生产构建 | 通过；dist 更新时间 10:58:03 |
| cargo build --workspace --release | 通过，用时 9 分 58 秒 |

扩大检查期间，core 单元测试为 593 通过、1 失败、3 跳过。失败为修改前已复现的 `cliproxy_general_aliases_keep_non_official_models_and_yield_to_special_official_channel`：期望专用官方 relay，却得到通用 relay。该项不属于本次凭据修复，没有修改其代码或跳过后声称全库通过；上述最终相关检查单独执行并通过。最初前端测试因默认 Node 20 不支持直接加载 `.ts` 被阻断，随后使用已安装的 Node 24 重跑通过，没有安装依赖。

## 发布产物

三个程序在 `target/release`、`bin` 和 `D:\pro\Codex++` 的 SHA-256 已逐一核对一致。

| 程序 | SHA-256 |
| --- | --- |
| codex-plus-plus-manager.exe | `0ACDB6770D86B20451214ACFC380B0B43809B686D15188F87F8E75B4B2241266` |
| codex-plus-plus.exe | `7E4B39247D8E43FCD4DA514B94E2CA2FD9A1E0536B2301DBB631CF144EA6BA16` |
| codex-plus-mobile-relay.exe | `0C22645ADF37A4C65E4462D196A95FEBF5F0347859A391B8A00B75AEEBA3B718` |

Manager 与启动器哈希均相对旧版改变，时间戳晚于前端构建，并在二进制中检查到了本次新增的刷新保护文本。mobile relay 不涉及本次修改，哈希保持原值。

部署先备份、暂存、核对哈希并执行单文件移动预览，再替换运行目录文件。旧程序保留在 `D:\pro\Codex++\backups\official-auth-fix-20261007-105455\`；工作区也保留原 bin 和运行目录的备份。

## 生效与实际登录验收

文件部署完成，但没有关闭承载当前聊天和微信连接的进程。复核时旧 Manager PID 为 21564，启动器 PID 为 4128，启动时间均早于新编译，因此它们仍运行旧代码。不能把文件哈希一致当作新进程验收完成。

当前活动官方账号仍为 `needsReauth`。本次没有重新执行真实 OAuth 登录或轮换用户令牌；单元测试的刷新请求使用测试凭据和模拟结果。失效的旧 refresh token 必须通过重新登录替换。

建议在结束当前聊天后：

1. 完整退出旧 Manager、Codex++ 启动器和桌面端；Manager 关闭窗口可能只是隐藏，使用托盘退出。
2. 从 `D:\pro\Codex++\codex-plus-plus-manager.exe` 启动新版本，对需要重新鉴权的账号重新登录。
3. 通过 Manager 启动 Codex，再刷新账号用量；核对官方会话正常打开、用量刷新成功，日志没有重复使用 refresh token 的错误。

真实重新登录、进程重启和官方网络请求仍待用户完成；本报告只确认源码修复、自动化回归、构建和文件部署。
