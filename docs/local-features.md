# CodexPlusPlus — 本地功能维护清单

本文件记录本 fork 相对于上游 BigPizzaV3/CodexPlusPlus origin/main 的核心差异功能。
每次上游合并后必须逐条确认回归，避免本地行为被上游简化行为覆盖。

---

## 1. 聚合供应商与模型路由（最大分叉）

本地扩展了聚合供应商的模型别名、路由、展示、映射管理和故障切换。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| 聚合模型别名 | `crates/codex-plus-core/src/aggregate_model_alias.rs` | 上游无此文件。成员别名、dispatch entries、catalog 别名全部在这里。合并后如果编译通过不代表逻辑正确。 |
| 聚合路由 | `crates/codex-plus-core/src/relay_rotation.rs` | `aggregate_member_pool_for_provider_alias`、`dispatch_entries`、aggregate failover 选择逻辑。上游行为完全不同，合并后需逐函数确认。 |
| 模型目录 | `crates/codex-plus-core/src/model_catalog.rs` | `aggregate_catalog_aliases` 调用、`displaySuffix` 注入、提供者独立模型条目（`供应商一:gpt-5.4`）生成。 |
| 聚合数据结构 | `crates/codex-plus-core/src/settings.rs` | `AggregateRelayProfile`、`AggregateRelayMember`、`AggregateRelayModelMapping`、`AggregateRelayDispatchTarget`。 |
| 前端聚合面板 | `apps/codex-plus-manager/src/aggregateMappings.ts` | 新文件。展示顺序、有效映射计算、提供者标签生成。 |
| 前端聚合编辑器 | `apps/codex-plus-manager/src/App.tsx` | `AggregateRelayProfileEditor`、`normalizeAggregateConfig`、`inferAggregateModelList`、`aggregateDisplayModelEntries`。 |
| 前端测试 | `apps/codex-plus-manager/src/aggregateMappings.test.ts` | 顺序回归测试。 |

**合并确认点**：检查聚合供应商保存→应用后，模型下拉是否出现带括号的正确名称、各成员 aliases 是否按顺序排列、mappings 编辑是否可保存恢复。

---

## 2. 认证与会话隔离

非官方供应商的 API key 写入 `experimental_bearer_token` 而非 `auth.json`，
切换供应商时不覆写官方 ChatGPT/Codex 登录态。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| 配置写入 | `crates/codex-plus-core/src/relay_config.rs` | `requires_openai_auth` 识别、token 路径选择、`save_relay_file` 拦截写 auth。 |
| 切换逻辑 | `crates/codex-plus-core/src/relay_switch.rs` | `save_backfill_profile_for` 参数变更，不再从 live config/auth 回填之前 provider。`backfill_relay_profile_from_live` 已移除。 |
| Manager 后端 | `apps/codex-plus-manager/src-tauri/src/commands.rs` | `save_relay_file` 限制、`sync_providers_now` 参数简化。 |
| 启动器 | `apps/codex-plus-launcher/src/main.rs` | 启动时调用会话归一。 |

**合并确认点**：切换纯 API 供应商后 `auth.json` 不应被覆写，`config.toml` 中 `experimental_bearer_token` 应正确存在且 `requires_openai_auth = false`。

---

## 3. 会话提供者归一（provider sync）

历史会话的 `model_provider` 元数据统一写入 `custom`，消除多 provider 残留导致的对话记录混乱。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| 核心逻辑 | `crates/codex-plus-data/src/provider_sync.rs` | `normalize_all_session_providers_to_custom()`、备份机制。 |
| 测试 | `crates/codex-plus-data/tests/provider_sync.rs` | 23 项测试覆盖 rollout/SQLite/backup/rollback。 |
| 公开 API | `crates/codex-plus-data/src/lib.rs` | 暴露 provider_sync。 |

**合并确认点**：执行"统一全部历史会话到 custom"后，rollout 与 SQLite 均更新为 `custom`，备份不受影响。

---

## 4. 插件市场保留

openai-curated/remote marketplace 配置写入时保留已有的第三方 marketplace 条目。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| 合并逻辑 | `crates/codex-plus-core/src/plugin_marketplace.rs` | `merge_marketplace_entries_with_locally_selected`、`keep_openai_curated_and_custom_entries`。 |
| 注入脚本 | `assets/inject/renderer-inject.js` | 插件自动展开关闭、backend settings 加载时序。 |

**合并确认点**：保存聚合供应商后，已有第三方 marketplace 插件配置不丢失。

---

## 5. 模型窗口与上下文窗口

源自上游 v1.2.41+ 的 feature，本地做了兼容修复。

| 模块 | 文件 | 维护要点 |
|---|---|---|
| 前端辅助 | `apps/codex-plus-manager/src/model-windows.test.ts` | modelWindows / modelVlm 字段完整性测试。 |
| 目录生成 | `crates/codex-plus-core/src/relay_config.rs` | `apply_context_limits_to_config`、`suffix stripping`、`model_catalog_json` 生成。 |

---

## 6. 其他本地调整

- `.gitignore` 增加 codex agent 忽略规则
- `Cargo.toml` 增加 `mobile-relay` workspace member
- 编译配置：`[profile.release]` 增加 `strip = true`、`lto = "fat"`、`codegen-units = 1`、`panic = "abort"`（减少二进制体积约 35%）
- `provider_import.rs`、`ccs_import.rs` 的细微兼容修补

---

## 上游合并流程

```powershell
# 1. 拉取上游
git fetch origin --prune

# 2. 以本地为主进行三方合并（非自动 rebase）
git merge --no-commit --no-ff origin/main

# 3. 逐模块确认以下文件未被上游覆盖
#    - crates/codex-plus-core/src/aggregate_model_alias.rs          （上游无此文件）
#    - crates/codex-plus-core/src/relay_rotation.rs                 （aggregate 函数）
#    - crates/codex-plus-core/src/model_catalog.rs                  （catalog 别名注入）
#    - crates/codex-plus-core/src/relay_config.rs                   （认证隔离）
#    - crates/codex-plus-core/src/relay_switch.rs                   （backfill 移除）
#    - crates/codex-plus-core/src/plugin_marketplace.rs             （保留逻辑）
#    - apps/codex-plus-manager/src/aggregateMappings.ts             （新文件）
#    - apps/codex-plus-manager/src/App.tsx                          （聚合面板）
#    - apps/codex-plus-manager/src-tauri/src/commands.rs            （save_relay_file 拦截）
#    - crates/codex-plus-data/src/provider_sync.rs                  （会话归一）

# 4. 运行关键测试
cargo test -p codex-plus-core --tests -- --test-threads=1
cargo test -p codex-plus-data --tests -- --test-threads=1
cargo test -p codex-plus-manager --lib -- --test-threads=1

# 5. TypeScript 检查与前端测试
cd apps/codex-plus-manager
../node_modules/.bin/tsc --noEmit -p tsconfig.json
node --test "src/*.test.ts"

# 6. 发布编译
cargo build --workspace --release
