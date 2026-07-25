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

## 发布编译

编译完成后将 Release 二进制复制到 `bin/`：

```powershell
# 编译
cargo build --workspace --release

# 复制到 bin/
Copy-Item -LiteralPath "target\release\codex-plus-plus-manager.exe" -Destination "bin\" -Force
Copy-Item -LiteralPath "target\release\codex-plus-plus.exe" -Destination "bin\" -Force
Copy-Item -LiteralPath "target\release\codex-plus-mobile-relay.exe" -Destination "bin\" -Force

# 验证
Get-ChildItem -LiteralPath "bin" -Filter "codex*" | ForEach-Object {
  $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
  Write-Output "$($_.Name)  SHA256: $hash"
}
```

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
cd apps/codex-plus-manager
.\node_modules\.bin\tsc --noEmit -p tsconfig.json
node --test "src/*.test.ts"

# 5. 生产构建
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
