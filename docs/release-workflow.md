# 后台检查、编译和交付

代码修改与耗时的发布流水线分开。Agent 完成改动、必要的针对性验证和分片组装后，启动一次后台任务即可结束本轮；不在对话中轮询等待。只有任务状态为 `succeeded` 才能称为构建及交付成功。

从仓库根目录启动：

```powershell
# 默认交付到仓库 bin/；前端改动传入能标识本次修改的文本/标记
pwsh -NoProfile -File .\scripts\release.ps1 -Action Start -ExpectedUiText '本次功能的独特文本'

# 已获授权更新现有安装目录时，显式指定目标
pwsh -NoProfile -File .\scripts\release.ps1 -Action Start -ExpectedUiText '本次功能的独特文本' -DeployTo 'D:\pro\Codex++'
```

启动命令返回任务编号、PID 和证据目录，随后独立的隐藏 PowerShell 进程继续工作。任务不依赖本轮聊天继续执行；不使用 `Start-Job`，不创建可见窗口。纯后端修改可省略 `-ExpectedUiText`，默认仍刷新 Vite 以避免嵌入旧资源。

需要查看进度时读取一次状态，不必持续等待：

```powershell
pwsh -NoProfile -File .\scripts\release.ps1 -Action Status
pwsh -NoProfile -File .\scripts\release.ps1 -Action Status -Job '返回的任务编号'
```

状态含义：`queued` 等待启动，`running` 正在执行，`succeeded` 完成检查及交付，`failed` 某项失败，`interrupted` 工作进程意外退出。状态中包含当前阶段、日志、错误、产物 SHA-256、备份和交付路径。检查 PID 时同时校验进程启动时间，避免 PID 复用造成误判。

脚本顺序执行：

1. 获取工作区独占锁并检查是否已有 Cargo/Rustc/Link/Tauri 编译进程，禁止重复任务。
2. 校验交接时的源码文件列表与 SHA-256，包括新建、删除文件；构建输入含 Rust、Manager、assets、scripts、内嵌图片及 Cargo 配置。
3. renderer 组装一致性、Rust fmt/check、core/data/Manager 测试、TypeScript 和前端测试。Rust 使用当前 Cargo、默认 `-j 2`，共享测试单线程。
4. 执行项目已有 Vite CLI（与 `npm run vite:build` 相同），检查 dist 更新时间与指定文本，再构建 Rust Release。不会调用会打安装包的 `npm run build`。
5. 等待编译进程结束，重新核对源码、dist、Manager 时间戳及三个产物 SHA-256。
6. 为全部目标先备份并核对，再交付到 bin/ 和显式指定的运行目录，逐文件核对最终哈希。失败会停止后续步骤；替换过程中失败时尝试恢复已替换的旧文件。
7. 落盘最终状态与摘要，列出当时运行的 Manager/启动器及实际路径，不结束或重启任何应用。

源码漂移、校验失败、UI 标记缺失、旧 Manager 产物或非零命令退出都会阻止交付。脚本不自动重编译或重试整轮发布；修复对应问题后再启动新任务。已在编译时继续修改源码，原任务会失败并保留日志，不能把它当作新改动的构建。

默认需要已安装的 Node 24+（运行 TS 测试）、Cargo、rg 和项目 node_modules；优先使用现有 Node，也会尝试当前用户已有的 Codex Node 运行时。可用 `-NodePath`、`-CargoPath` 明确已有工具路径，`-Jobs` 设置并行度。不安装依赖、不切换 Rust 工具链。

证据保存在 `output/release-jobs/<任务编号>/`：

- `request.json`：交接参数、冻结输入清单。
- `status.json`、`summary.md`：状态与交付结论；`frontend.json`：本次 dist 清单。
- 各步骤 `.log`，以及进程启动阶段的 `worker.out.log`、`worker.err.log`。
- `backup-bin/`：仓库 bin 旧文件；运行目录旧文件在其 `backups/release-<任务编号>/`。

不自动删除日志、备份和未完成的中间文件。首次交付到原本没有文件的 bin 时，若交付中途失败，已新建的文件保留供检查；状态仍为失败。意外终止时也不得视为交付完成，应根据清单核对恢复。脚本不读取或修改真实 config/auth、账号设置及聊天数据库，不提交、不推送。应用重启及界面验收仍由用户安排。

只修改脚本/文档时无需重跑产品 Release。验证脚本流程用隔离假工具：

```powershell
pwsh -NoProfile -File .\scripts\release.tests.ps1
```

该检查验证后台返回、空格路径、并发锁、成功交付及备份哈希、默认只更新 bin、失败停止、源码新增/dist 变化检测、UI 缺失/旧 Manager 拦截和遗留状态识别；不会调用真实 Cargo/Node 构建，也不会修改真实安装目录。测试目录保留在 output 下。
