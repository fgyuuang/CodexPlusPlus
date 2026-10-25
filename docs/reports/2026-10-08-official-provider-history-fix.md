# 官方账号切换后历史会话 provider 丢失

日期：2026-10-08。目标对话：`01a11ace-65f5-7913-a794-56db911459f9`。

## 原因与现场恢复

目标对话的 SQLite 记录保存了账号专用 `codex_plus_official_<账号 ID>` provider，以及相应的 `provider:gpt-6-luna` 模型。账号变为当前主账号后，菜单目录用 `openai` 提供其原生模型，旧的账号专用 provider 不再属于当前菜单来源。配置更新据此删除其定义，导致恢复历史会话时报 `Model provider ... not found`。代理的模型路由也仅从当前菜单来源查找，所以只补配置仍不能保证旧会话请求正常。

现场已从历史备份恢复目标 provider 定义，先备份至用户 Codex home 下的 `config.toml.bak.20261008-205327`，再原子写入。用 Python 3.12 的 TOML 解析器验证了配置，确认默认 provider 和其他配置未被此次恢复修改。没有修改登录凭据、SQLite、rollout 或会话的模型归属。目标账号元数据为启用且 `ready`，其缓存目录包含 `gpt-6-luna`。

## 源码修复

- `relay_config.rs`：官方体验配置为已保存账号保持稳定的 provider 定义，并保留配置中既有的历史官方 provider。主账号改变、目录暂不可用或账号失效都不再触发这些表的清理；根 provider 仍为 `openai`。
- `official_experience.rs`：当前菜单中没有匹配项时，历史官方 provider 根据原账号 ID 与该账号可信目录解析。账号必须存在、启用且 `ready`，完整模型别名必须与账号匹配。不会按当前活动账号的模型集合删减原账号的模型，也不会把未知模型发送给第三方。
- `protocol_proxy.rs`：账号专用图像入口的模型解析同样使用历史路由解析器，维持原账号的认证边界。
- 更新 `provider-auth-session-switching.md`，记录当前菜单与历史会话 provider 的不同生命周期。

新增两项回归测试，覆盖账号变为主账号、切换到目录不同的另一主账号、错误账号别名、未知模型、账号失效或禁用，以及连续刷新配置仍保留历史 provider。路由测试使用临时账号库与目录缓存，未轮换真实账号令牌。

## 验证

- 官方相关最终单元测试：48 通过。显式排除了此前已复现的独立 CLIProxy 优先级失败项，没有将其计为通过。
- 协议代理 / 配置 / 切换集成测试：150 / 185 / 24 全部通过，共 359 项。
- `cargo check --workspace`、`cargo fmt --all -- --check` 与改动文件的空白检查通过。
- 已确认前端 dist 时间晚于 App.tsx，未修改前端源码。

源码备份、源码 SHA-256、测试日志和部署脚本位于 `output/official-provider-history-20261008-205326/`。

## 发布与实际验收

发现本机另有一份 21:02:15 开始的 `cargo build --workspace --release`，没有启动重复构建。其 core 编译开始于 21:02:18，晚于本次源码修改；等待期间复查源码 SHA-256 未改变。通过保留该 cargo 进程句柄观察到构建退出码为 0。新 Manager 更新时间为 21:12:09，启动器为 21:07:27。

三个程序已更新到 `bin` 和 `D:\pro\Codex++`，并逐一确认与 `target/release` 的哈希一致。移动前执行了单文件预览，旧程序保留在 `D:\pro\Codex++\backups\official-provider-history-20261008-205326\`。没有关闭当前桌面端、Manager 或微信连接。

| 程序 | SHA-256 |
| --- | --- |
| codex-plus-plus-manager.exe | `D4501F7BBF3D671389887B156D24F87894DC68F9235F7734AB7A5E13CBFFE459` |
| codex-plus-plus.exe | `E5EC6680CA1AAAD58B1894BAE8B98272F368B0A2028C00ADB71DC6044D4C19E1` |
| codex-plus-mobile-relay.exe | `0C22645ADF37A4C65E4462D196A95FEBF5F0347859A391B8A00B75AEEBA3B718` |

Manager 和启动器哈希均相对部署前改变；mobile relay 不涉及此次修改，哈希保持原值。

仍需实际验收：完整退出旧 Manager、启动器及 Codex 桌面端，从 `D:\pro\Codex++\codex-plus-plus-manager.exe` 启动新版本，再重新打开目标对话。只有重新加载新进程后，账号切换与目录刷新才会使用保留历史 provider 的新逻辑。现在确认的是现场配置恢复、源码回归、成功构建和文件部署，尚未验证该对话的实际继续请求。
