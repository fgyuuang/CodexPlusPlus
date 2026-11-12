# 本地服务插件化实施记录

**最终发布结果：2026-11-05 09:47:53（北京时间）成功完成编译、备份与替换，任务 `20261105-085252-0de45070` 为 `succeeded/complete`。独立核验 35 项产物/交付/备份哈希全部一致，4 项账号相关文件哈希未变。当前运行验收仍待用户重启新版后确认。**

用户要求将本地 CLI/API 集成转为插件。以已合并的正式 v2.2.0 为底座，采用上游 manager.page/nav 和沙箱桥接；未追入新的上游提交，没有重写官方调度或服务实现。

## 当前改动

CLIProxyAPI 与 NewAPI 各自成为 bundled 管理器插件，页面和操作编排位于包内，工具侧栏及旧连接页默认打开插件。共享模板在发布时内联为每个包自包含的 manager.html；插件清单、说明与页面可单独维护，无 npm 依赖。

宿主只增加两项精确声明的管理器能力，复用既有类型化服务适配。每次调用复核插件启用与权限，不扩展 renderer/HTTP 公共路由，不开放任意 Tauri 或 shell。返回字段采用白名单，去掉完整 settings 与明文密钥，显示密钥是否已保存。新密钥继续保存于既有 DPAPI 存储。

原服务路径、账号文件、供应商 ID 与连接保持原地兼容。没有读取或搬迁外部账号，没有启动、停止、下载服务，没有修改 config.toml/auth.json。停用插件阻止后续 CLI 自启动，不停止正在运行的独立服务或删除供应商。旧版兼容入口留待真实验收和回滚；这次完成的是页面/编排的插件化，安全服务代码仍为原生宿主适配。

官方登录、多账号调度、子代理来源、图片和历史隔离继续保留核心边界。WorkBuddy/Claudian 继续采用上游 Agent 接入；仓库没有独立 FastAPI 模块，普通此类 API 连接采用供应商池，暂不创建重复路径。

## 定向验证

- `cargo test -p codex-plus-core --test all local_ -- --test-threads=1`：80 通过，含两包物化/权限隔离和清单/启用状态复核。记录 `output/plugin-core-targeted.log`。
- `cargo test -p codex-plus-manager --lib local_integrations:: -- --test-threads=1`：2 通过，覆盖秘密字段投影和未授权/未知/格式错误的拒绝。记录 `output/plugin-native-targeted.log`。
- Node 24 面板及桥接测试：11 通过；最终面板再验证 4 通过。覆盖只调用状态、明确保存/测试/接入、模型文本渲染、失败解锁和密钥保存后清空。记录 `output/plugin-panel-targeted.log`、`output/plugin-panel-final.log`。
- TypeScript 检查通过，记录 `output/plugin-typescript.log`。完整发布仍须对冻结后的最终输入重新检查，定向测试不代替发布门禁。
- 全仓 scope 默认扫描受既有大规模上游合并与历史输出影响，预算警告未冒充通过。模块边界、预算复核、回滚及未改动范围见 [执行契约](../plans/2026-11-05-local-integration-plugins.md)。

## 发布及验收

后台任务 `20261105-085252-0de45070` 已成功交付到 `bin/` 与已授权的 `D:\pro\Codex++`，状态证据为 `output/release-jobs/20261105-085252-0de45070/status.json`。完整测试结束后才进入 Vite 与 Rust Release，再做备份和哈希交付。启动前保存了 config/auth/官方账号及加密密钥库四项哈希，记录 `output/pluginization-protected-before-release.json`，完成后独立核对均未变化。

运行验收待用户安排重启后确认：工具侧栏两插件可打开，原连接/密钥状态正确，模型刷新与显式接入可用；停用后面板操作被拒绝，CLI 不再自启动；原官方账号、历史、网关连接保持可用。本轮没有对用户的实际服务执行启停来代替这项验收。

## 独立包后续修改（2026-11-05 09:19）

原发布任务当时已通过 renderer、Rust 格式/检查、core/data/Manager 全量测试、TypeScript、前端测试与 Vite，为 `running/release`；当时正式编译及交付仍未完成。前端 1618 通过、2 跳过，Manager 182 通过、1 忽略，data 308 通过；各步骤退出码为 0。未重新启动构建。

为了不破坏冻结输入，在其范围外新增 `local-plugins/manage.mjs`、隔离测试和说明。支持独立打包、哈希校验、默认预览安装、备份更新及用备份回装。实际用户目录已完成只读预览，没有安装、启用插件或修改用户配置。7 项 Node 隔离测试通过，记录 `output/local-plugin-packages-tests.log`。

已生成可脱离原仓库使用的维护包：`output/local-plugin-kit-20261105-091758/`，包含源码、工具、两个自包含插件包及说明；其工具已用维护包内的独立 source 路径实际打包验证。后续复制该目录到单独仓库即可维护面板，无需 Cargo/Vite；新增原生能力仍须宿主适配。

检查时原发布的 1339 个已冻结文件哈希均未改变，config/auth/官方账户及加密密钥库 4 项哈希也未改变。默认连接页入口及 CLI 自启动仍绑定 bundled key，已列为后续兼容边界；不能把独立包工具完成等同于全部宿主改动已移除。见 [独立包门禁](../plans/2026-11-05-local-plugin-packages.md) 和 `local-plugins/README.md`。

## 最终交付核对（2026-11-05 10:00）

- 全部发布步骤退出码 0。core 4007 通过、34 忽略；data 308 通过、2 忽略；Manager 182 通过、1 忽略；前端 1618 通过、2 跳过。没有失败用例。
- 正式优化 Release 用时 29 分 35 秒，完整任务约 55 分钟；09:47:53 完成备份和替换。交付三个 EXE 与四个运行库 DLL，到 bin 和安装目录。
- 构建产物 7 项、两个目标交付 14 项、旧版备份 14 项，合计 35 项 SHA256 独立核对全部一致。证据 `output/release-jobs/20261105-085252-0de45070/independent-verification.json`。
- Manager SHA256：`3F6F52E64E2BECAC6C59739221863D7F3FE72F85CBCC90A0D13B3155835C8DEE`。安装备份在 `D:\pro\Codex++\backups\release-20261105-085252-0de45070`。
- config.toml、auth.json、官方账号元数据及加密密钥库四项哈希未变。没有剩余 Cargo/Rustc/Link 编译进程。
- 没有自动重启应用。核对时 Manager PID 31664 仍为 08:38 启动的旧进程，文件替换不会更新已加载代码。请用户保存工作后自行退出并重开 Codex++ 与 Codex，再验证插件入口和原有账号/会话。
- 独立维护包已生成且测试通过；未向生产 user_scripts 安装额外副本、未改启用配置，以免重复入口。当前发布包含第一阶段 bundled 插件，独立包及自启动兼容边界仍按上文分别验收。
