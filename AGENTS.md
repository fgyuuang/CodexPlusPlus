# AGENTS.md

本文件为 CodexPlusPlus fork 的工作规范，指导 agent 在本仓库工作。

## 项目概述

本仓库是 [BigPizzaV3/CodexPlusPlus](https://github.com/BigPizzaV3/CodexPlusPlus) 的 fork。
当前版本 v1.2.42，本地分支 `codex/fix-plugin-marketplace-persistence`。

本地扩展了聚合供应商路由与模型别名、认证与会话隔离、插件市场保留等功能，
详见 `docs/local-features.md` — 每次上游合并后必须逐条确认回归。

## 仓库结构

- `crates/codex-plus-core/` — 核心 Rust 库（配置生成、catalog 解析、数据模型）
- `apps/codex-plus-manager/` — Tauri 桌面应用，前端 React+TS
- `crates/codex-plus-data/` — 数据持久化
- `docs/` — 本地功能维护清单、设计文档、调研、计划

## 关键代码位置

- 聚合模型别名：`crates/codex-plus-core/src/aggregate_model_alias.rs`
- 聚合路由：`crates/codex-plus-core/src/relay_rotation.rs`
- 模型目录：`crates/codex-plus-core/src/model_catalog.rs`
- 配置生成与人证隔离：`crates/codex-plus-core/src/relay_config.rs`
- 切换逻辑：`crates/codex-plus-core/src/relay_switch.rs`
- 插件市场保留：`crates/codex-plus-core/src/plugin_marketplace.rs`
- 会话提供者归一：`crates/codex-plus-data/src/provider_sync.rs`
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

## 发布编译

```powershell
cargo build --workspace --release
```

Release 配置已在 `Cargo.toml` 中启用 `strip = true`、`lto = "fat"`、`codegen-units = 1`、`panic = "abort"`。

## 与上游同步

- `origin` = https://github.com/BigPizzaV3/CodexPlusPlus（fetch）
- `fork` = https://github.com/fgyuuang/CodexPlusPlus.git（push）
- 工作分支：`codex/fix-plugin-marketplace-persistence`
- 合并策略：`git merge --no-commit --no-ff origin/main`，**不以 rebase 方式处理**，保留完整本地提交历史
- 合并后逐文件核对 `docs/local-features.md` 中标记的模块
- 关键回归测试后提交并推送 fork 分支
