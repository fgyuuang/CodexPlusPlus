# 本地 vs 线上（fork/上游）差异对照

- 取证时间：2026-11-15
- 工作副本：`D:\pro\codexpp-v269-review-20261114`（worktree，分支 `feat/reset-cards-2.6.10`，HEAD `b8dd3208b`）
- 远端：`fork` = https://github.com/fgyuuang/CodexPlusPlus.git　上游 `origin` = BigPizzaV3/CodexPlusPlus，tag `v2.6.10` = `bf41f0a3d`
- 基线：`git diff v2.6.10 HEAD` = **496 文件，+114950 / −30887**

## 结论先行

1. **没有落后上游。** `git merge-base --is-ancestor v2.6.10 HEAD` 成立，且 `git rev-list --count HEAD..v2.6.10` = **0**。`3fefa8b65 merge: 整合上游 v2.6.10` 已把 v2.6.10 完整吞进来，所以这 496 个文件的差量**全部是本地侧**，不含任何"上游有而我们没有"的内容。
2. **推上去的到底是什么。** 61 个提交里，只有 **6 个**是真正做在 2.6.10 基线之上的（重置卡 + 2 个网关修复 + 3 个测试稳定性），另有 3 个合并后杂项。其余 **52 个是 fork 从 v1.2.x 起累积的历史**——它们之所以出现在 `v2.6.10..HEAD` 里，是因为这条本地线一直没被上游吸收，而不是这次新写的。
3. **重置卡的真实体量：8 个文件 / +688 行 / 0 删除**（`b8dd3208b`），占全部增行 114950 的 **0.6%**。它的核心文件 `gateway/reset_credits.rs` 与 2.4.0 分支 `448fbe18f` 是**同一个 blob**（`11f8324640`），即逐字节原样移植，没有为 2.6.10 重写。
4. **含什么**：整套本地功能（多官方账号调度、官方体验路由、聚合模型别名、渠道保护、本地代理、按工具分区的配置与 UI、CLIProxy/CLI 相关、重置卡）、99 个 core 测试文件、43 篇本地文档、重构过的 inject 注入层。
5. **不含什么**：① 上游 v2.6.10 之后的任何提交；② 2.4.0 分支独有的 `build-stamp.ts`（生成物，未移植）；③ `site/` 静态站（60 个文件整体删除）、`tools/claude-extension-demo`、`.github/workflows/image-overlay-regressions.yml`；④ 2.4.0 分支本身（两条线没有合并，是逐文件搬过来的）。
6. **数字有水分**：`assets/inject/renderer-inject.js` 一个文件就 +17403/−11653，它是 `scripts/assemble-renderer-inject.mjs` 拼出来的**单体产物**（1.7MB），内容等于 `assets/inject/renderer-inject/` 下 20 个分片之和。真实源码增量应扣掉这 ~29000 行的重复计数。

## 表 1 提交层差异（`v2.6.10..HEAD`，61 个）

| 类别 | 提交数 | 代表提交 | 说明 |
| --- | --- | --- | --- |
| 本地功能（含本地缺陷修复） | 33 | `ee40028e8`、`90e0004a9`、`39eec3813`、`2d74edf02`、`b8dd3208b` | 账号调度/官方体验/聚合别名/Grok 与工具分区/重置卡；跨 v1.2.35→v2.6.10 整条历史 |
| 合并上游 + 合并后修补 | 10 | `3fefa8b65`、`97b4ff422`、`9831b8bb3`、`291ce3e07`、`b267aae40` | 9 个 merge（v1.2.42/54/56、v2.0.0、v2.6.10 等）+ 1 个合并后收尾 |
| 测试稳定性 | 6 | `8046da79e`、`cef390507`、`4f7eba360`、`4416dd877`、`59244fef9`、`fd80257e9` | 并行跑时的顺序/预缓存/作用域问题，不改进产品行为 |
| 杂项（快照、chore、style、docs） | 12 | `5a8394d5c`、`4c6818f42`、`97dfefefe`、`841f22337`、`cc4d4a163` | 含 3 个"整合前快照"；`841f22337` 误带入 `output/` 产物、`cc4d4a163` 随即撤回（净零） |
| **合计** | **61** | | 其中 **9 个**（6 功能修复 + 3 杂项）位于 `3fefa8b65` 之后 |

## 表 2 文件层差异（按目录）

| 目录 | 文件数 | + | − |
| --- | --- | --- | --- |
| `crates/codex-plus-core/src` | 164 | 46647 | 7372 |
| `crates/codex-plus-core/tests` | 99 | 17176 | 4758 |
| `apps/codex-plus-manager/src` | 51 | 13489 | 4333 |
| `apps/codex-plus-manager/src-tauri` | 21 | 7297 | 613 |
| `assets/inject`（含单体产物） | 21 | 23383 | 11841 |
| `docs` | 43 | 3237 | 8 |
| `site`（整目录删除） | 60 | 0 | 1439 |
| `crates/codex-plus-data` | 6 | 1481 | 262 |
| `assets`（extensions + 元数据） | 9 | 220 | 4 |
| 构建/脚本/插件（`scripts`、`local-plugins`、`tools`、`.github`） | 10 | 1078 | 125 |
| `apps/codex-plus-launcher` | 3 | 596 | 71 |
| 根目录与清单（`Cargo.*`、`README`、`AGENTS.md` 等） | 9 | 346 | 61 |
| **合计** | **496** | **114950** | **30887** |

新增 96 个文件、删除 64 个文件。`crates/codex-plus-core/src` 里最大的三项是 `gateway/official_accounts.rs`（+508）、`gateway/agents/claudian.rs`（+434）、`gateway/quota.rs`（+396/−117）。

## 表 3 重置卡功能的确切边界（相对 v2.6.10）

| 文件 | 该文件净变化 | 其中属于重置卡 | 改了什么 |
| --- | --- | --- | --- |
| `crates/codex-plus-core/src/gateway/reset_credits.rs`（新增） | +373 / −0 | **373（全部）** | 查 `wham/rate-limit-reset-credits`、按 `/consume` 幂等消费、内存缓存 + 60s 陈旧判定；与 2.4.0 同 blob |
| `crates/codex-plus-core/src/gateway/mod.rs` | +3 / −1 | **1** | 仅 `pub mod reset_credits;`；另 2 行是 `official_accounts` 声明与 `agents` 排序，属别的本地功能 |
| `apps/codex-plus-manager/src-tauri/src/gateway_commands.rs` | +324 / −117 | **+59** | `GatewayPoolEntry.reset_credits` 字段、`pool_entry()` 注入缓存、`refresh_gateway_reset_credits` 与 `consume_gateway_reset_card` 两个命令；其余 265 行是多工具账号 / native auth 等 |
| `apps/codex-plus-manager/src-tauri/src/lib.rs` | +135 / −28 | **2** | 仅两行 `invoke_handler` 注册 |
| `apps/codex-plus-manager/src/GatewayProviders.tsx` | +120 / −3 | **110** | `ResetCreditsRow`（刷新、确认后消费、逐张明细、未确认重试提示）+ `ResetCardsChip`（折叠态张数）+ 两处挂载与 import |
| `apps/codex-plus-manager/src/gateway-model.ts` | +41 / −0 | **39** | `ResetCredit`/`ResetCredits` 类型、`usableResetCardCount`、`soonestResetCardExpiry`；另 2 行是无关的 `redacted` 字段 |
| `apps/codex-plus-manager/src/GatewayPage.css` | +81 / −0 | **81（全部）** | `.gw-reset-chip` / `.gw-reset-value` / `.gw-reset-list` 等样式 |
| `apps/codex-plus-manager/src/i18n-en.ts` | +522 / −1 | **23** | 重置卡文案（"重置券 N"、确认弹窗、错误提示等）；其余 499 行是其它功能的文案 |

**净边界：+688 行、0 删除、8 个文件。** 判定方法：只用 `reset_credit|ResetCard|ResetCredits|重置券|重置卡|refresh_gateway_reset_credits|consume_gateway_reset_card` 匹配 `git diff -U0` 的行，再与 `git show --stat b8dd3208b`（688 insertions）交叉核对，两者一致。

## 表 4 2.4.0 树 vs v2.6.10 树的功能缺口

对比 `448fbe18f`（主仓库 2.4.0 线）与 tag `v2.6.10` 的模块清单：2.4.0 有 **10 个** core 顶层模块 + 3 个 `gateway/` 模块在上游 v2.6.10 里**完全不存在**；本分支 HEAD 把这些全部补了回来。

| 只在 2.4.0 存在的关键文件 | 作用 | v2.6.10 | 本分支 HEAD |
| --- | --- | --- | --- |
| `gateway/reset_credits.rs` | 额度重置卡 | 无 | 有，同 blob `11f8324640` |
| `gateway/official_accounts.rs` | 网关侧官方账号池 | 无 | 有，同 blob `5025fac0` |
| `gateway/agents/claudian.rs` | Claudian 代理接入 | 无 | 有，内容已随上游演进不同（`151192aa` vs `0345acca`） |
| `official_accounts.rs` + `official_account_schedule.rs` | 多官方账号与调度 | 无 | 有，同 blob |
| `official_child_routing.rs` + `official_experience.rs` | 官方体验多来源路由 | 无 | 有，同 blob |
| `official_model_catalog.rs` + `aggregate_model_alias.rs` | 动态官方模型目录、聚合别名 | 无 | 有，同 blob |
| `channel_protection.rs` | 渠道保护 | 无 | 有，同 blob |
| `protocol_proxy_local.rs` + `launcher_local_proxy.rs` | 本地协议代理 / 启动代理 | 无 | 有，同 blob |
| `codex_app_tools_compat.rs` | Codex App 工具兼容 | 无 | 有，同 blob |
| 前端 `official-usage.ts`、`official-model-catalog.ts`、`aggregateMappings.ts` 等 20 个 | 官方用量/目录/聚合映射 UI | 无 | 有（仅缺 `build-stamp.ts`，生成物） |

反向缺口（v2.6.10 有、2.4.0 无）：`gateway/connection_generation.rs`、`claude_local_backend.rs`、`config_doctor.rs` 等上游新模块 —— 这些在本分支 HEAD 里同样存在，属于上游正常演进。

**结论**：2.4.0 线相对 v2.6.10 的功能缺口，在本分支上**已全部补齐，且绝大多数是字节级相同的原样搬运**（12 个关键文件里 11 个 blob 一致）。唯一内容有差异的是 `gateway/agents/claudian.rs` 与整体被上游重写过的 `quota.rs` / `route.rs` / `apply.rs`（这些文件同时承载上游改动，不能简单按"本地新增"计）。

## 附：可复现命令

```bash
cd D:/pro/codexpp-v269-review-20261114
git merge-base --is-ancestor v2.6.10 HEAD && echo ancestor
git rev-list --count HEAD..v2.6.10          # 0 → 没落后
git log --oneline v2.6.10..HEAD             # 61
git diff --shortstat v2.6.10 HEAD           # 496 / +114950 / −30887
git show --stat b8dd3208b                   # 重置卡：8 文件 +688
git diff -U0 v2.6.10 HEAD -- <file> | grep -E "reset|Reset|重置|consume"
```
