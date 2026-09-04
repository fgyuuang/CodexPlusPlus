# AGENTS.md

本文件为 CodexPlusPlus fork 的工作规范，指导 agent 在本仓库工作。

## 项目概述

本仓库是 [BigPizzaV3/CodexPlusPlus](https://github.com/BigPizzaV3/CodexPlusPlus) 的 fork。
当前版本 v1.2.42，本地分支 `codex/fix-plugin-marketplace-persistence`。

本地扩展了聚合供应商路由与模型别名、认证与会话隔离、插件市场保留等功能，
详见 `docs/local-features.md` —— 每次上游合并后必须逐条确认回归。

## 仓库结构

- `crates/codex-plus-core/` —— 核心 Rust 库（配置生成、catalog 解析、数据模型）
- `apps/codex-plus-manager/` —— Tauri 桌面应用，前端 React+TS
- `crates/codex-plus-data/` —— 数据持久化
- `docs/` —— 本地功能维护清单、设计文档、调研、计划
- `bin/` —— 已编译的 Release 二进制，由 `.gitignore` 排除

## 关键代码位置

- 聚合模型别名：`crates/codex-plus-core/src/aggregate_model_alias.rs`
- 聚合路由：`crates/codex-plus-core/src/relay_rotation.rs`
- 模型目录：`crates/codex-plus-core/src/model_catalog.rs`
- 配置生成与认证隔离：`crates/codex-plus-core/src/relay_config.rs`
- 切换逻辑：`crates/codex-plus-core/src/relay_switch.rs`
- 插件市场保留：`crates/codex-plus-core/src/plugin_marketplace.rs`
- 会话提供者归一：`crates/codex-plus-data/src/provider_sync.rs`
- 会话 provider 统计与切换策略：`crates/codex-plus-data/src/provider_sync.rs` 的 `load_provider_sync_targets`、`provider_sync_target_for_settings`
- 前端聚合面板与映射：`apps/codex-plus-manager/src/aggregateMappings.ts`
- 前端聚合编辑器：`apps/codex-plus-manager/src/App.tsx` 的 `AggregateRelayProfileEditor`
- 数据模型：`crates/codex-plus-core/src/settings.rs` 的 `RelayProfile` 结构体
- 配置生成：`crates/codex-plus-core/src/relay_config.rs` 的 `apply_context_limits_to_config`
- catalog 解析：`crates/codex-plus-core/src/model_catalog.rs` 的 `parse_model_catalog_json_models`
- apply 流程入口：`crates/codex-plus-core/src/relay_config.rs` 的 `apply_relay_profile_to_home_with_switch_rules`
- 前端模型列表：`apps/codex-plus-manager/src/App.tsx` 的 `modelList` textarea

## 安全规则

- 禁止批量删除、rm -rf、rmdir /s
- 删除只能单个文件，删除前确认
- 禁止 sudo、提权、curl | bash
- 禁止泄露密钥、.env、auth.json、config.toml 凭据
- 覆盖文件前确认
- 不擅自改 Cargo.toml、package.json、.gitignore（除非任务必需）

## 命令执行

- 执行 bash 命令前确认
- 不运行未知脚本、不擅自装依赖
- 测试用 cargo test，不另起工具链

## 编码规范

- 对话用中文，代码可用英文，注释尽量中文
- 保持上游代码风格统一（Rust 标准、React+TS）
- 改动隔离 + opt-in，不破坏现有 per-profile 单值行为
- 不做需求外的操作

## 测试约定

- 沿用上游 `#[test]` + tempfile 风格
- 断言读 config.toml 文本，如 `assert!(config.contains("model_catalog_json"))`
- 改行为要同步改/加对应测试
- Windows 下共享路径的集成测试使用 `--test-threads=1`
- provider sync 必须同时覆盖 `openai` 与 `custom`：纯官方活动配置同步到 `openai`，聚合、独立 API、官方混入 API 同步到 `custom`
- 会话管理必须显示各 provider 的唯一会话数、rollout 数和 SQLite 数；不得再次退化为固定同步到 `custom`
- 官方登录混合模式必须保持认证源与请求目标分离：`officialLoginRelayId` 只恢复官方 ChatGPT 登录，第三方或聚合 profile 负责 API 覆写；官方 API 不得加入聚合成员、轮转或权重计算
- 官方登录混合模式下，官方原生模型保持原名并优先显示；聚合替换项使用半角格式 `gpt-5.4(供应商1|供应商2:真实模型)`；目标模型与 Codex 模型相同时只显示供应商名称
- 官方登录混合模式的裸模型名只允许来自可信官方清单：`gpt-5.6-sol`、`gpt-5.6-terra`、`gpt-5.6-luna`、`gpt-5.5`、`gpt-5.4`、`gpt-5.4-mini`、`gpt-5.3-codex`；供应商提供的其他 `gpt-*` 不得伪装成官方裸模型
- 官方裸模型必须直连 ChatGPT Codex Responses，官方请求失败或 WebSocket 握手失败不得进入供应商轮转；聚合只接受括号别名或 `供应商:模型`，未知裸模型必须关闭式拒绝
- 官方登录混合模式的 `model_provider = "custom"` 是本地协议代理的传输标识，不代表官方裸模型被发往第三方；不得仅为界面显示把混合会话强制改写成 `openai`，否则恢复会话可能绕过聚合代理
- 官方内置 `image_gen` 的 `/v1/images/generations` 与 `/v1/images/edits` 必须使用所选 ChatGPT 登录直通 `https://chatgpt.com/backend-api/codex/images/*`；不得使用 `experimental_bearer_token`、供应商轮转或 failover，也不得把请求正文、提示词、图像数据或认证头写入日志
- 由 CodexPlusPlus 生成并指向本地 HTTP 协议代理的 model catalog 必须设置 `prefer_websockets = false`，防止 Codex 对不支持 WebSocket 的本地代理重复握手后误入供应商路径
- Codex 模型列表内切换模型时，必须在新版 `electronBridge.sendMessageFromView` 请求层校验目标模型的 reasoning effort；官方 `gpt-5.6-sol/terra/luna` 在目录尚未加载时仍须保留各自内置能力，供应商模型不得继承不支持的 `max/ultra`
- 流式供应商或聚合请求失败、非 2xx、断流或缺少终止事件时，协议代理必须返回合法 `response.failed` SSE 并记录 `helper.protocol_proxy_stream_failed`；不得把异常流记录为成功，也不得因此导致 thread agent loop 死亡

## 发布编译

Manager 是 Tauri 应用，前端资源来自 `apps/codex-plus-manager/dist/`。只运行
`cargo build --workspace --release` 不会可靠地执行 Vite 构建，可能把旧 `dist`
嵌入新的 Manager 可执行文件，表现为后端命令已更新但页面仍是旧版。

涉及 Manager 前端改动时必须先单独刷新 `dist`，再编译 Rust 工作区。不要使用
`npm run build` 代替下面的两阶段流程：该脚本还会执行完整 `tauri build`，耗时更长，
并可能生成当前验收不需要的安装包。

```powershell
# 1. 生成最新前端资源
Set-Location -LiteralPath "apps\codex-plus-manager"
npm run vite:build

# 2. 确认 dist 时间戳已更新，并按本次功能选择一个独特的新界面文本进行检查
Get-Item -LiteralPath "dist\index.html" | Select-Object FullName, LastWriteTime, Length
$bundle = Get-ChildItem -LiteralPath "dist\assets" -Filter "index-*.js" -File |
  Sort-Object LastWriteTime -Descending |
  Select-Object -First 1
$bundleText = [System.IO.File]::ReadAllText($bundle.FullName, [System.Text.Encoding]::UTF8)
if (-not $bundleText.Contains("本次新增的独特界面文本")) {
  throw "最新前端改动未进入 dist，禁止继续发布编译。"
}

# 3. 回到仓库根目录并编译；Release 优化可能需要 10-20 分钟
Set-Location -LiteralPath "..\.."
cargo build --workspace --release

# 4. 仅在所有 cargo/rustc/tauri 子进程结束且产物时间戳已刷新后复制到 bin/
Copy-Item -LiteralPath "target\release\codex-plus-plus-manager.exe" -Destination "bin\" -Force
Copy-Item -LiteralPath "target\release\codex-plus-plus.exe" -Destination "bin\" -Force
Copy-Item -LiteralPath "target\release\codex-plus-mobile-relay.exe" -Destination "bin\" -Force

# 5. 验证 target/release 与 bin 哈希逐一一致
$artifacts = @(
  "codex-plus-plus-manager.exe",
  "codex-plus-plus.exe",
  "codex-plus-mobile-relay.exe"
)
foreach ($name in $artifacts) {
  $releaseHash = (Get-FileHash -LiteralPath (Join-Path "target\release" $name) -Algorithm SHA256).Hash
  $binHash = (Get-FileHash -LiteralPath (Join-Path "bin" $name) -Algorithm SHA256).Hash
  if ($releaseHash -ne $binHash) { throw "SHA-256 mismatch: $name" }
  Write-Output "$name  $binHash"
}
```

Windows 下命令超时不代表构建已经停止；`node`、`tauri`、`cargo` 或 `rustc` 子进程
可能仍在后台运行。超时后先用 `Get-CimInstance Win32_Process` 检查相关进程，等待原构建
结束，禁止直接启动第二份重复构建。最终至少确认以下证据：

- 新界面文本已进入最新 `dist/assets/index-*.js`。
- Release 可执行文件的 `LastWriteTime` 已晚于前端构建时间，且 SHA-256 相比旧产物改变。
- `target/release` 与 `bin` 中对应文件的 SHA-256 完全一致。
- 从 `bin` 复制或移动到其他目录的旧副本不会自动更新；验收时必须确认实际启动路径，
  并完全退出旧 Manager 后再启动新产物。

`bin/` 已被 `.gitignore` 排除，不纳入版本控制。

Release 配置已在 `Cargo.toml` 中启用 `strip = true`、`lto = "fat"`、`codegen-units = 1`、`panic = "abort"`。

## 空间管理

`target/debug` 易积累到 50 GB+，建议定期清理：

```powershell
cargo clean          # 清除所有编译缓存
# 或只清 debug 保留 release
Remove-Item -LiteralPath "target\debug" -Recurse -Force
```

## 验收流程

提交前依次完成以下步骤，确认无误后由用户确认：

```powershell
# 1. 检查未提交文件范围
git status --short

# 2. Rust 检查
cargo fmt --all -- --check
cargo check --workspace

# 3. 关键回归测试（单线程避免 Windows 共享冲突）
cargo test -p codex-plus-core --tests -- --test-threads=1
cargo test -p codex-plus-data --tests -- --test-threads=1
cargo test -p codex-plus-manager --lib -- --test-threads=1

# 4. TypeScript 检查与前端测试
Push-Location -LiteralPath "apps\codex-plus-manager"
.\node_modules\.bin\tsc --noEmit -p tsconfig.json
node --test "src/*.test.ts"
Pop-Location

# 5. 生产构建（前端改动必须先刷新 dist，详见“发布编译”）
Push-Location -LiteralPath "apps\codex-plus-manager"
npm run vite:build
Pop-Location
cargo build --workspace --release

# 6. 复制到 bin/
Copy-Item -LiteralPath "target\release\codex-plus-plus-manager.exe" -Destination "bin\" -Force
Copy-Item -LiteralPath "target\release\codex-plus-plus.exe" -Destination "bin\" -Force
Copy-Item -LiteralPath "target\release\codex-plus-mobile-relay.exe" -Destination "bin\" -Force

# 7. 出示 SHA-256 供验收确认
Get-ChildItem -LiteralPath "bin" -Filter "codex*" | ForEach-Object {
  $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
  Write-Output "$($_.Name)  $hash"
}
```

用户确认无误后执行：

```powershell
git add --update
git commit -m "合并描述"
git push -u fork codex/fix-plugin-marketplace-persistence
```

## 与上游同步

- `origin` = https://github.com/BigPizzaV3/CodexPlusPlus（fetch）
- `fork` = https://github.com/fgyuuang/CodexPlusPlus.git（push）
- 工作分支：`codex/fix-plugin-marketplace-persistence`
- 合并策略：`git merge --no-commit --no-ff origin/main`，不以 rebase 方式处理，保留完整本地提交历史
- 合并后逐文件核对 `docs/local-features.md` 中标记的模块并运行回归测试
- 上游若修改登录、`model_provider`、启动顺序、会话索引或 SQLite schema，必须人工确认 `docs/provider-auth-session-switching.md` 的状态机仍成立
