# Codex 侧边栏推荐入口移除

## 范围

- 用户要求移除 Codex 侧边栏的推荐，保留 Manager 推荐，并在编译后清理过程文件。
- `40-backend-settings.js` 不再注册推荐导航按钮，每次扫描会移除旧版本残留的推荐节点。
- 保留已发布的推荐 ID、页面及兼容映射，不更名第三方接口；Manager 概览推荐和推荐页保持原样。
- 前一次布局与重启按钮修复保留；未修改供应商、认证、账号、会话数据库或配置。

## 验证

- 已重新组装 18 个 renderer 分片，产物 SHA-256：`93eb66778019d7208fb55543742dfda7e1893f6946485f8a37ff54289b5ef517`。
- 注入专项测试 99 项通过，包含推荐入口不再注册、旧节点清理和 Manager 推荐仍存在的检查。
- 当前 Codex 窗口重新注入后的实际 DOM：推荐节点不存在，主页与拓展节点存在；根容器宽度与视口一致。
- fmt 和 workspace check 通过。首次默认并发编译测试遇到 Windows 页面文件不足（os error 1455），改为单并发重试。
- 单并发重试：core 1,541 项、data 195 项、Manager 100 项通过；前端 379 项通过、1 项跳过，TypeScript 检查通过。
- Vite 生产资源于 18:39:43 更新，已检查重启按钮文本进入最新 bundle；组装一致性检查通过。
- 验证日志为 `output/sidebar-clean-*-20261004.log`（首次）及 `output/sidebar-retry-*-20261004.log`（单并发重试）。
- 工作区 Release 单并发构建通过，用时 12 分 59 秒；三个可执行文件时间戳均晚于 Vite 资源更新，复制前确认 cargo/rustc 全部退出。

## 产物

新包：`output/sidebar-clean-v1.5.0-20261004/`。旧 bin 已备份至 `output/sidebar-recommendation-backup-20261004/`，哈希验证通过。
`target/release`、`bin` 和新包中的三个文件 SHA-256 完全一致；安装脚本已通过语法解析和预览。

| 产物 | 构建时间 | SHA-256 |
|---|---|---|
| Manager | 18:52:28 | `ABA62155AA5A56C5726AA4C7616FD0CA1085AADCF3E035DAD7BC530A02315C23` |
| launcher | 18:49:05 | `0FCE1C8EF7FE82E6B38E3870C9C30231C7BF45A86E4C49DCFA216FBF5A1975A0` |
| mobile relay | 18:52:43 | `0C22645ADF37A4C65E4462D196A95FEBF5F0347859A391B8A00B75AEEBA3B718` |

当前窗口已生效。`D:\pro\Codex++` 中的旧 Manager 与 launcher 仍在运行，未中断用户任务或替换运行中的文件。
退出这两个程序后运行新包的 `Install-Update.ps1 -Apply`，可备份并安装新产物。没有提交或推送仓库。

## 清理计划

仅删除本次及前次 UI 诊断产生的一次性脚本和临时 renderer 副本。逐文件核对绝对路径、长度与哈希，清单记录于 `output/sidebar-cleanup-preview-20261004.json`。
保留源码与二进制备份、更新包、验收日志和恢复聊天导出；不递归清理输出目录或其他任务文件。

构建和打包结束后逐文件完成清理，共 11 个文件；结果及原文件哈希保存于 `output/sidebar-cleanup-completed-20261004.json`。
清理内容包括临时 DOM 检查脚本、CDP 诊断脚本、两份临时 renderer 副本及一次性验证/打包脚本。
