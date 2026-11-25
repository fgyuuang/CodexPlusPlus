//! 仅迁移用户当前打开、已由原生接口卸载的一个旧 Codex++ 会话；不运行全库同步。
use super::*;
use anyhow::{Context, ensure};
use codex_plus_core::gateway::{route, store::GatewaySettings};
use codex_plus_core::settings::RelayProfile;
use codex_plus_core::tools::ToolId;

pub fn migrate_gateway_thread_provider(
    home: &Path,
    thread_id: &str,
    source: &str,
    target: &str,
    model: &str,
    gateway: &GatewaySettings,
    managed_profile: Option<&RelayProfile>,
) -> anyhow::Result<Value> {
    ensure!(uuid::Uuid::parse_str(thread_id).is_ok(), "会话 ID 无效");
    ensure!(
        source.starts_with("codex_plus_official_") && ["openai", "custom"].contains(&target),
        "只允许旧官方来源迁移到当前网关传输"
    );
    let binding = gateway
        .active_binding(&ToolId::Codex)
        .context("Codex 网关尚未接入")?;
    ensure!(
        route::codex_pool_models(gateway, binding)
            .iter()
            .chain(std::iter::once(&binding.model))
            .any(|known| known == model),
        "目标模型不在当前网关目录中"
    );
    let config_path = home.join("config.toml");
    let config_text = fs::read_to_string(&config_path)?;
    let config: toml::Value =
        toml::from_str(&config_text).map_err(|_| anyhow::anyhow!("当前 Codex 配置无法解析"))?;
    ensure!(
        config
            .get("model_provider")
            .and_then(toml::Value::as_str)
            .unwrap_or("openai")
            == target,
        "当前来源已经变化，停止迁移"
    );
    let endpoint = if target == "openai" {
        config
            .get("openai_base_url")
            .and_then(toml::Value::as_str)
            .or_else(|| {
                config
                    .get("model_providers")?
                    .get(target)?
                    .get("base_url")?
                    .as_str()
            })
    } else {
        config
            .get("model_providers")
            .and_then(|p| p.get(target))
            .and_then(|p| p.get("base_url"))
            .and_then(toml::Value::as_str)
    };
    let local_url = |text: &str, path: &str| {
        text.strip_prefix("http://")
            .and_then(|rest| rest.split_once('/'))
            .is_some_and(|(authority, actual_path)| {
                let valid_address = ["127.0.0.1:", "localhost:", "[::1]:"].iter().any(|host| {
                    authority
                        .strip_prefix(host)
                        .is_some_and(|port| port.parse::<u16>().is_ok_and(|port| port > 0))
                });
                valid_address && format!("/{}", actual_path.trim_end_matches('/')) == path
            })
    };
    // 官方登录的原生入口经本机协议代理，再转到活动网关 profile。
    // 必须确认实际活动 profile 及同一监听地址，不能仅凭 localhost 放行。
    let managed_proxy = endpoint.is_some_and(|url| {
        ["/v1", "/v1/routes/openai"].iter().any(|path| {
            local_url(url, path)
                && managed_profile.is_some_and(|profile| {
                    let upstream = if profile.upstream_base_url.trim().is_empty() {
                        &profile.base_url
                    } else {
                        &profile.upstream_base_url
                    };
                    profile.id == codex_plus_core::gateway::apply::CODEX_GATEWAY_RELAY_ID
                        && profile.relay_mode == codex_plus_core::settings::RelayMode::PureApi
                        && local_url(upstream, "/gw/codex/v1")
                        && url.trim_end_matches('/').strip_suffix(*path)
                            == upstream.trim_end_matches('/').strip_suffix("/gw/codex/v1")
                })
        })
    });
    ensure!(
        managed_proxy || endpoint.is_some_and(|url| local_url(url, "/gw/codex/v1")),
        "当前配置未指向本机 Codex 网关"
    );
    ensure!(
        config
            .get("model_providers")
            .and_then(|p| p.get(source))
            .and_then(|p| p.get("base_url"))
            .and_then(toml::Value::as_str)
            .is_some_and(|url| local_url(url, &format!("/v1/routes/{source}"))),
        "旧 provider 不是本机受管来源，未迁移"
    );
    let _guard = acquire_lock(&home.join("tmp/provider-sync.lock"))?;
    ensure!(
        pending_recovery_journal(home).is_none(),
        "有未完成的会话修复，请先恢复该作业"
    );
    let databases = provider_sync_db_paths(home);
    let mut rollouts = HashSet::new();
    let mut found = false;
    for path in &databases {
        if !path.is_file() {
            continue;
        }
        let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let columns = table_columns(&db, "threads")?;
        if !["id", "model_provider", "rollout_path", "source"]
            .iter()
            .all(|key| columns.contains(*key))
        {
            continue;
        }
        let row: Option<(String, String, String)> = db
            .query_row(
                "SELECT model_provider,rollout_path,source FROM threads WHERE id=?1",
                [thread_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((provider, rollout, origin)) = row {
            ensure!(
                provider == source || provider == target,
                "会话来源在迁移前发生变化"
            );
            ensure!(
                !origin.to_ascii_lowercase().contains("subagent"),
                "子代理由根对话管理来源，不单独迁移"
            );
            rollouts.insert(
                migration_rollout_path(home, &rollout).context("找不到此会话的本机活动记录")?,
            );
            found = true;
        }
    }
    ensure!(
        found && rollouts.len() == 1,
        "无法唯一确认此会话记录，未迁移"
    );
    let rollout = rollouts.into_iter().next().unwrap();
    let compressed_sha = if rollout.extension().is_some_and(|ext| ext == "zst") {
        Some(validate_compressed_index_repair(
            &rollout, thread_id, target,
        )?)
    } else {
        None
    };
    let changes = if compressed_sha.is_some() {
        Vec::new()
    } else {
        let (id, providers) =
            rollout_provider_state_for_path(&rollout)?.context("会话记录缺少来源元数据")?;
        ensure!(
            id == thread_id
                && !providers.is_empty()
                && providers.iter().all(|p| p == source || p == target),
            "会话记录与索引来源不一致，未迁移"
        );
        let changes = collect_session_change_for_path(&rollout, target, source, thread_id)?.changes;
        ensure!(changes.len() == 1, "会话文件被占用，未迁移");
        let changes: Vec<_> = changes
            .into_iter()
            .filter(|change| change.rewrite_needed)
            .collect();
        changes
    };
    let backup = create_backup(home, target, &changes)?;
    let mut journal = SyncJournal::new(&backup)?;
    journal.prepare_rollouts(
        changes
            .iter()
            .map(|change| crate::sync_journal::JournalRollout {
                path: change.path.clone(),
                recovery_path: None,
                before_sha256: format!("{:x}", Sha256::digest(change.original_text.as_bytes())),
                after_sha256: format!("{:x}", Sha256::digest(change.next_text.as_bytes())),
                original_meta_lines: change.original_session_meta_lines.clone(),
                original_mtime: change.original_mtime,
                applied: None,
                restored: false,
            })
            .collect(),
    )?;
    let migrated = (|| -> anyhow::Result<usize> {
        ensure!(
            fs::read_to_string(&config_path)? == config_text,
            "配置在备份期间改变，未迁移"
        );
        if let Some(expected) = &compressed_sha {
            ensure!(sha256_file(&rollout)? == *expected, "压缩会话在迁移前改变");
        }
        let applied = apply_session_changes(&changes)?;
        ensure!(
            applied.skipped_locked_rollout_files.is_empty()
                && applied.changes.len() == changes.len(),
            "会话在迁移前被修改或占用"
        );
        journal.mark_rollouts_applied(
            &applied
                .changes
                .iter()
                .map(|change| change.path.clone())
                .collect::<Vec<_>>(),
        )?;
        let mut rows = 0;
        for path in &databases {
            if !path.is_file() {
                continue;
            }
            let mut db = Connection::open(path)?;
            db.busy_timeout(std::time::Duration::from_millis(250))?;
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let before = SqliteSnapshot::capture(&tx)?;
            for (table, key) in [("threads", "id"), ("local_thread_catalog", "thread_id")] {
                let columns = table_columns(&tx, table)?;
                if !columns.contains(key) || !columns.contains("model_provider") {
                    continue;
                }
                let provider: Option<String> = tx
                    .query_row(
                        &format!("SELECT model_provider FROM {table} WHERE {key}=?1"),
                        [thread_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                ensure!(
                    provider
                        .as_deref()
                        .is_none_or(|p| p == source || p == target),
                    "索引来源在迁移期间改变"
                );
                rows += tx.execute(
                    &format!(
                        "UPDATE {table} SET model_provider=?1 WHERE {key}=?2 AND model_provider=?3"
                    ),
                    (target, thread_id, source),
                )?;
            }
            journal.commit(tx, before, path)?;
        }
        ensure!(
            if let Some(expected) = &compressed_sha {
                sha256_file(&rollout)? == *expected
            } else {
                rollout_file_matches_provider(&rollout, thread_id, target)?
            },
            "会话来源写入后核验失败"
        );
        journal.complete()?;
        Ok(rows)
    })();
    match migrated {
        Ok(rows) => Ok(
            json!({"status":"ok", "threadId":thread_id, "modelProvider":target,
            "model":model, "backupDirectory":backup, "changedSessionFiles":changes.len(), "sqliteRowsUpdated":rows}),
        ),
        Err(error) => {
            let recovery = journal.rollback();
            anyhow::bail!(
                "单会话来源迁移失败：{error}；恢复错误数：{}；备份：{}",
                recovery.len(),
                backup.display()
            )
        }
    }
}

fn migration_rollout_path(home: &Path, value: &str) -> Option<PathBuf> {
    resolve_active_rollout_path(home, value).or_else(|| {
        let path = Path::new(value.trim());
        if path.extension().is_none_or(|ext| ext != "jsonl") {
            return None;
        }
        let mut compressed = path.as_os_str().to_os_string();
        compressed.push(".zst");
        resolve_active_rollout_path(home, &compressed.to_string_lossy())
    })
}

/// 只修索引的情形：压缩历史自身已是目标来源，原件不重写、不解压落盘。
fn validate_compressed_index_repair(path: &Path, id: &str, target: &str) -> anyhow::Result<String> {
    const MAX_BYTES: u64 = 256 * 1024 * 1024;
    ensure!(
        path.to_string_lossy().ends_with(".jsonl.zst"),
        "不是受支持的压缩历史"
    );
    let before = sha256_file(path)?;
    let decoder = zstd::stream::read::Decoder::new(File::open(path)?)?;
    let mut text = String::new();
    decoder.take(MAX_BYTES + 1).read_to_string(&mut text)?;
    ensure!(text.len() as u64 <= MAX_BYTES, "压缩历史超出安全读取上限");
    let (actual_id, providers) =
        rollout_thread_provider_state(&text).context("压缩历史缺少会话来源元数据")?;
    ensure!(
        actual_id == id && providers.len() == 1 && providers.contains(target),
        "压缩历史仍需修改来源或身份不一致，未重写；请先通过原生客户端恢复记录"
    );
    ensure!(sha256_file(path)? == before, "压缩历史在读取期间改变");
    Ok(before)
}
