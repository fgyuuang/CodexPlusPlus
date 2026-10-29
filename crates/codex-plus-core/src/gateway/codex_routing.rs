//! Codex 网关的来源边界：可信父子谱系、已确认根请求的供应商和官方裸模型。
//! 只记本进程已确认的来源，不猜测重启前或尚未发出请求的 UI 选择。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;

use super::route::{self, RoutePlan};
use super::store::GatewaySettings;
use crate::protocol_proxy::ProxySessionHeaders;
use crate::settings::BackendSettings;
use crate::tools::ToolId;

const AFFINITY_LIMIT: usize = 2048;
const AFFINITY_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    root: String,
    child: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    id: String,
    #[serde(default)]
    session_id: Option<String>,
    model_provider: String,
    ephemeral: bool,
    parent_thread_id: Option<String>,
    forked_from_id: Option<String>,
    #[serde(default)]
    is_subagent: bool,
}

struct Affinity {
    relay_id: Option<String>,
    generation: u64,
    at: Instant,
}

fn affinities() -> &'static Mutex<HashMap<String, Affinity>> {
    static MAP: OnceLock<Mutex<HashMap<String, Affinity>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn uuid(value: &str) -> anyhow::Result<String> {
    Ok(uuid::Uuid::parse_str(value.trim())
        .context("Codex 网关线程标识无效，未推测根对话")?
        .to_string())
}

/// 非 UUID 的独立 CLI 会话键仅用于缓存，绝不当作原生线程身份。
fn request_ids(headers: ProxySessionHeaders<'_>) -> anyhow::Result<Vec<String>> {
    let mut ids = Vec::new();
    if let Some(id) = headers.thread_id {
        ids.push(uuid(id)?);
    }
    if let Some(id) = headers.session_id {
        match uuid(id) {
            Ok(id) if !ids.contains(&id) => ids.push(id),
            Ok(_) => {}
            Err(_) if headers.thread_id.is_none() => {}
            Err(error) => return Err(error),
        }
    }
    Ok(ids)
}

fn persisted(home: &Path, id: &str) -> anyhow::Result<Option<Metadata>> {
    let mut found: Option<Metadata> = None;
    for path in crate::codex_sqlite::codex_thread_reference_db_paths_from_home(home) {
        if !path.is_file() {
            continue;
        }
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.busy_timeout(Duration::from_millis(100))?;
        let has_threads: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='threads')",
            [],
            |row| row.get(0),
        )?;
        if !has_threads {
            continue;
        }
        let metadata: Option<(String, String)> = db
            .query_row(
                "SELECT source,model_provider FROM threads WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((source, provider)) = metadata else {
            continue;
        };
        let has_edges: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='thread_spawn_edges')",
            [], |row| row.get(0),
        )?;
        let parents = if has_edges {
            db.prepare(
                "SELECT DISTINCT parent_thread_id FROM thread_spawn_edges WHERE child_thread_id=?1",
            )?
            .query_map([id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        ensure!(
            parents.len() <= 1,
            "Codex 网关子代理父线程不唯一，停止来源继承"
        );
        let next = Metadata {
            id: id.to_string(),
            session_id: Some(id.to_string()),
            model_provider: provider,
            ephemeral: false,
            parent_thread_id: parents.into_iter().next(),
            forked_from_id: None,
            is_subagent: source.to_ascii_lowercase().contains("subagent"),
        };
        if let Some(previous) = &found {
            ensure!(
                previous.model_provider == next.model_provider
                    && previous.parent_thread_id == next.parent_thread_id
                    && previous.is_subagent == next.is_subagent,
                "Codex 网关线程记录冲突，停止来源继承"
            );
        }
        found = Some(next);
    }
    Ok(found)
}

async fn native(id: &str) -> anyhow::Result<Metadata> {
    let port = crate::status::StatusStore::default()
        .load_latest()?
        .and_then(|status| status.debug_port)
        .context("原生线程尚未持久化，也没有受信只读查询入口")?;
    let targets = crate::cdp::list_targets(port).await?;
    let target = crate::cdp::pick_injectable_codex_page_target(&targets)?;
    let websocket = target
        .web_socket_debugger_url
        .context("原生线程查询缺少调试地址")?;
    let script = format!(
        "(async()=>{{const read=window.__codexPlusReadNativeThreadMetadata;if(typeof read!==\"function\")throw new Error(\"native metadata reader unavailable\");return JSON.stringify(await read({}));}})()",
        serde_json::to_string(id)?
    );
    let response =
        crate::bridge::evaluate_script_with_await_promise(&websocket, &script, true).await?;
    let value = response
        .pointer("/result/result/value")
        .and_then(serde_json::Value::as_str)
        .context("原生线程只读查询未返回身份")?;
    let metadata: Metadata = serde_json::from_str(value)?;
    ensure!(metadata.id == id, "原生线程查询返回了其他身份");
    Ok(metadata)
}

async fn identity_with<F, Fut>(
    home: &Path,
    ids: Vec<String>,
    mut reader: F,
) -> anyhow::Result<Option<Identity>>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Metadata>>,
{
    let mut resolved: Option<Identity> = None;
    for id in ids {
        let mut current = id.clone();
        let mut seen = HashSet::new();
        loop {
            ensure!(
                seen.len() < 64 && seen.insert(current.clone()),
                "Codex 网关父子关系有环或超过限制"
            );
            let metadata = match persisted(home, &current)? {
                Some(metadata) => metadata,
                None => reader(current.clone()).await?,
            };
            ensure!(metadata.id == current, "Codex 网关线程身份查询不匹配");
            if let Some(session) = &metadata.session_id {
                ensure!(uuid(session)? == current, "原生线程的 session 身份存在冲突");
            }
            let parent = metadata.parent_thread_id.as_deref().or_else(|| {
                metadata
                    .ephemeral
                    .then_some(metadata.forked_from_id.as_deref())
                    .flatten()
            });
            if let Some(parent) = parent {
                current = uuid(parent)?;
                continue;
            }
            ensure!(
                !metadata.is_subagent,
                "Codex 网关子代理缺少可信父关系，未推测来源"
            );
            ensure!(
                matches!(metadata.model_provider.as_str(), "custom" | "openai"),
                "根对话不是当前 Codex 网关入口，停止来源继承"
            );
            let route = Identity {
                root: current,
                child: seen.len() > 1,
            };
            if let Some(previous) = &resolved {
                ensure!(
                    previous.root == route.root,
                    "请求标识属于不同根对话，停止来源继承"
                );
            }
            resolved = Some(Identity {
                child: route.child || resolved.as_ref().is_some_and(|old| old.child),
                ..route
            });
            break;
        }
    }
    Ok(resolved)
}

pub(crate) async fn identity(
    home: &Path,
    headers: ProxySessionHeaders<'_>,
) -> anyhow::Result<Option<Identity>> {
    let ids = request_ids(headers)?;
    tokio::time::timeout(
        Duration::from_secs(6),
        identity_with(home, ids, |id| async move { native(&id).await }),
    )
    .await
    .context("Codex 网关原生谱系查询超时，未推测来源")?
}

pub(crate) fn conversation(identity: &Identity) -> &str {
    &identity.root
}

fn remembered(identity: &Identity) -> anyhow::Result<String> {
    let mut entries = affinities()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    entries.retain(|_, value| value.at.elapsed() < AFFINITY_TTL);
    entries
        .get(&identity.root)
        .and_then(|value| value.relay_id.clone())
        .context(
            "尚未确认根对话的网关来源；请先在根对话发起一次请求恢复绑定，不会猜测账号或自动改派",
        )
}

pub(crate) fn begin_root(identity: &Identity) -> Option<u64> {
    if identity.child {
        return None;
    }
    static GENERATION: AtomicU64 = AtomicU64::new(1);
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    let mut entries = affinities()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    entries.retain(|_, value| value.at.elapsed() < AFFINITY_TTL);
    if entries.len() >= AFFINITY_LIMIT {
        if let Some(oldest) = entries
            .iter()
            .min_by_key(|(_, value)| value.at)
            .map(|(key, _)| key.clone())
        {
            entries.remove(&oldest);
        }
    }
    entries.insert(
        identity.root.clone(),
        Affinity {
            relay_id: None,
            generation,
            at: Instant::now(),
        },
    );
    Some(generation)
}

pub(crate) fn remember_root(identity: &Identity, generation: u64, relay_id: &str) {
    if identity.child {
        return;
    }
    let mut entries = affinities()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(entry) = entries
        .get_mut(&identity.root)
        .filter(|entry| entry.generation == generation)
    {
        entry.relay_id = Some(relay_id.to_string());
    }
}

fn fixed_child_plan(
    gateway: &GatewaySettings,
    requested: &str,
    identity: &Identity,
) -> anyhow::Result<RoutePlan> {
    let selected = remembered(identity)?;
    let binding = gateway
        .active_binding(&ToolId::Codex)
        .context("Codex 网关未接入")?;
    let relay = route::binding_pool_relays(gateway, binding)
        .into_iter()
        .find(|relay| relay.id == selected)
        .context("根对话的供应商已停用或移除，子代理不会改派其他来源")?;
    let (request_slug, _) = crate::model_suffix::parse_model_suffix(requested.trim());
    let requested = request_slug.trim();
    let base = requested
        .split_once('/')
        .filter(|(id, _)| gateway.provider(id).is_some())
        .map_or(requested, |(_, model)| model);
    ensure!(
        route::relay_models(relay).iter().any(|model| model == base),
        "根对话来源没有子代理所请求的模型，未猜测模型或改派供应商"
    );
    let mut isolated = gateway.clone();
    isolated
        .providers
        .retain(|provider| provider.relay.id == selected);
    isolated.groups.clear();
    isolated.auto_models = false;
    let mut plan = route::resolve_plan(
        &isolated,
        &ToolId::Codex,
        &format!("{selected}/{base}"),
        Some(&identity.root),
    )?;
    ensure!(
        plan.candidates
            .iter()
            .all(|candidate| candidate.relay.id == selected),
        "子代理来源校验失败"
    );
    plan.source_model = requested.to_string();
    // 官方账号没有第二 Key/账号 fallback；API 来源仍可在同供应商内正常轮换 Key。
    if crate::official_login::member_uses_official_login_transport(relay) {
        plan.candidates.truncate(1);
    }
    Ok(plan)
}

/// 压缩属于已确认对话的延续，不能把内部裸模型名当作新的账号选择。
pub(crate) fn continuation_plan(
    gateway: &GatewaySettings,
    requested: &str,
    identity: &Identity,
) -> anyhow::Result<RoutePlan> {
    fixed_child_plan(gateway, requested, identity)
}

/// 有原生子谱系的官方图片只沿已确认的根来源；不能使用另一个全局生图账号。
pub(crate) fn child_image_target(
    gateway: &GatewaySettings,
    identity: &Identity,
) -> anyhow::Result<Option<super::images::ImageTarget>> {
    if !identity.child {
        return Ok(None);
    }
    ensure!(
        !gateway
            .image_model_for(&ToolId::Codex)
            .trim()
            .eq_ignore_ascii_case(super::images::IMAGE_OFF),
        "Codex 生图已关闭"
    );
    let selected = remembered(identity)?;
    let binding = gateway
        .active_binding(&ToolId::Codex)
        .context("Codex 网关未接入")?;
    let relay = route::binding_pool_relays(gateway, binding)
        .into_iter()
        .find(|relay| relay.id == selected)
        .context("根来源已停用，子代理生图不改派账号")?;
    ensure!(
        crate::official_login::member_uses_official_login_transport(relay),
        "父聊天来源为 API，子代理官方生图需要可确认的官方来源；不会改用另一个账号或第三方"
    );
    Ok(Some(super::images::ImageTarget {
        relay: relay.clone(),
        model: None,
    }))
}

/// 只对 Codex 应用此边界；其它 Agent 的网关自动组保持原语义。
pub(crate) fn plan(
    settings: &BackendSettings,
    gateway: &GatewaySettings,
    requested: &str,
    conversation: Option<&str>,
    identity: Option<&Identity>,
) -> anyhow::Result<RoutePlan> {
    let trusted =
        crate::official_model_catalog::trusted_official_model_slugs_for_settings(settings);
    plan_with_models(gateway, requested, conversation, identity, &trusted)
}

fn plan_with_models(
    gateway: &GatewaySettings,
    requested: &str,
    conversation: Option<&str>,
    identity: Option<&Identity>,
    trusted: &[String],
) -> anyhow::Result<RoutePlan> {
    if let Some(identity) = identity.filter(|identity| identity.child) {
        return fixed_child_plan(gateway, requested, identity);
    }
    let mut plan = route::resolve_plan(gateway, &ToolId::Codex, requested, conversation)?;
    let (request_slug, _) = crate::model_suffix::parse_model_suffix(requested.trim());
    if trusted.iter().any(|model| model == request_slug.trim()) {
        plan.candidates.retain(|candidate| {
            crate::official_login::member_uses_official_login_transport(&candidate.relay)
                && candidate.upstream_model == request_slug.trim()
        });
        let sources: HashSet<_> = plan
            .candidates
            .iter()
            .map(|candidate| candidate.relay.id.as_str())
            .collect();
        ensure!(
            sources.len() == 1,
            "官方裸模型需要唯一的 ChatGPT 网关来源；多账号时请选择明确的供应商/模型，未尝试第三方"
        );
        plan.candidates.truncate(1);
        plan.group_id = None;
        plan.sticky_ttl = None;
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::store::{AgentBinding, GatewayProvider};
    use crate::settings::{RelayMode, RelayProfile};

    fn id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn provider(id: &str, official: bool) -> GatewayProvider {
        let relay = if official {
            RelayProfile {
                id: id.to_string(), name: id.to_string(), relay_mode: RelayMode::Official,
                auth_contents: serde_json::json!({"tokens":{"access_token":"fixture","refresh_token":"fixture-refresh","account_id":id}}).to_string(),
                model_list: "trusted-model\nother-model".to_string(), ..RelayProfile::default()
            }
        } else {
            RelayProfile {
                id: id.to_string(),
                name: id.to_string(),
                relay_mode: RelayMode::PureApi,
                base_url: format!("https://{id}.invalid/v1"),
                api_key: "fixture-key".to_string(),
                model_list: "trusted-model\nother-model".to_string(),
                ..RelayProfile::default()
            }
        };
        serde_json::from_value(serde_json::json!({"relay":relay})).unwrap()
    }

    fn gateway() -> GatewaySettings {
        let mut gateway = GatewaySettings::default();
        gateway.providers = vec![provider("api", false), provider("official", true)];
        gateway.bindings.insert(
            "codex".to_string(),
            AgentBinding {
                enabled: true,
                model: "trusted-model".to_string(),
                ..AgentBinding::default()
            },
        );
        gateway
    }

    async fn resolve(home: &Path, ids: Vec<String>) -> anyhow::Result<Option<Identity>> {
        identity_with(home, ids, |_| async {
            anyhow::bail!("unknown native identity")
        })
        .await
    }

    fn database(home: &Path) -> Connection {
        let db = Connection::open(home.join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,source TEXT,model_provider TEXT); CREATE TABLE thread_spawn_edges(parent_thread_id TEXT,child_thread_id TEXT);").unwrap();
        db
    }

    #[tokio::test]
    async fn verified_nested_children_follow_only_the_latest_confirmed_root_source() {
        let home = tempfile::tempdir().unwrap();
        let db = database(home.path());
        let (root, child, nested, other) = (id(), id(), id(), id());
        for (id, source) in [
            (&root, "cli"),
            (&child, "subagent"),
            (&nested, "subagent"),
            (&other, "cli"),
        ] {
            db.execute(
                "INSERT INTO threads VALUES(?1,?2,'custom')",
                [id.as_str(), source],
            )
            .unwrap();
        }
        for (parent, child) in [(&root, &child), (&child, &nested)] {
            db.execute(
                "INSERT INTO thread_spawn_edges VALUES(?1,?2)",
                [parent, child],
            )
            .unwrap();
        }
        let root_route = resolve(home.path(), vec![root.clone()])
            .await
            .unwrap()
            .unwrap();
        let child_route = resolve(home.path(), vec![nested.clone(), root.clone()])
            .await
            .unwrap()
            .unwrap();
        assert!(child_route.child);
        assert_eq!(child_route.root, root);
        assert!(
            remembered(&child_route).is_err(),
            "restart/unknown source is never guessed"
        );
        let older = begin_root(&root_route).unwrap();
        let newer = begin_root(&root_route).unwrap();
        remember_root(&root_route, newer, "api");
        remember_root(&root_route, older, "official");
        let gateway = gateway();
        let plan = plan_with_models(
            &gateway,
            "official/trusted-model",
            None,
            Some(&child_route),
            &["trusted-model".into()],
        )
        .unwrap();
        assert!(
            plan.candidates
                .iter()
                .all(|candidate| candidate.relay.id == "api")
        );
        assert_eq!(
            remembered(&child_route).unwrap(),
            "api",
            "old requests cannot overwrite a newer source"
        );
        assert!(
            resolve(home.path(), vec![nested, other]).await.is_err(),
            "cross-chat headers are rejected"
        );
        let next = begin_root(&root_route).unwrap();
        assert!(
            remembered(&child_route).is_err(),
            "new root work invalidates its previous source immediately"
        );
        remember_root(&root_route, next, "official");
        let plan = plan_with_models(
            &gateway,
            "trusted-model",
            None,
            Some(&child_route),
            &["trusted-model".into()],
        )
        .unwrap();
        assert_eq!(plan.candidates.len(), 1);
        assert_eq!(plan.candidates[0].relay.id, "official");
    }

    #[tokio::test]
    async fn missing_ambiguous_or_cyclic_relations_never_create_affinity() {
        let home = tempfile::tempdir().unwrap();
        let db = database(home.path());
        let (child, a, b) = (id(), id(), id());
        for (id, source) in [(&child, "subagent"), (&a, "cli"), (&b, "cli")] {
            db.execute(
                "INSERT INTO threads VALUES(?1,?2,'custom')",
                [id.as_str(), source],
            )
            .unwrap();
        }
        assert!(resolve(home.path(), vec![child.clone()]).await.is_err());
        assert!(resolve(home.path(), vec![id()]).await.is_err());
        db.execute("INSERT INTO thread_spawn_edges VALUES(?1,?2)", [&a, &child])
            .unwrap();
        db.execute("INSERT INTO thread_spawn_edges VALUES(?1,?2)", [&b, &child])
            .unwrap();
        assert!(resolve(home.path(), vec![child.clone()]).await.is_err());
        db.execute(
            "DELETE FROM thread_spawn_edges WHERE parent_thread_id=?1",
            [&b],
        )
        .unwrap();
        db.execute("INSERT INTO thread_spawn_edges VALUES(?1,?2)", [&child, &a])
            .unwrap();
        assert!(resolve(home.path(), vec![child]).await.is_err());
    }

    #[tokio::test]
    async fn native_ephemeral_identity_is_required_and_persistent_forks_remain_independent() {
        let home = tempfile::tempdir().unwrap();
        let root = id();
        let auxiliary = id();
        let fork = id();
        let snapshots = HashMap::from([
            (
                root.clone(),
                Metadata {
                    id: root.clone(),
                    session_id: Some(root.clone()),
                    model_provider: "custom".into(),
                    ephemeral: false,
                    parent_thread_id: None,
                    forked_from_id: None,
                    is_subagent: false,
                },
            ),
            (
                auxiliary.clone(),
                Metadata {
                    id: auxiliary.clone(),
                    session_id: Some(auxiliary.clone()),
                    model_provider: "custom".into(),
                    ephemeral: true,
                    parent_thread_id: None,
                    forked_from_id: Some(root.clone()),
                    is_subagent: false,
                },
            ),
            (
                fork.clone(),
                Metadata {
                    id: fork.clone(),
                    session_id: Some(fork.clone()),
                    model_provider: "custom".into(),
                    ephemeral: false,
                    parent_thread_id: None,
                    forked_from_id: Some(root.clone()),
                    is_subagent: false,
                },
            ),
        ]);
        let read =
            |id: String| std::future::ready(snapshots.get(&id).cloned().context("not found"));
        let proven = identity_with(home.path(), vec![auxiliary], read)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(proven, Identity { root, child: true });
        let read =
            |id: String| std::future::ready(snapshots.get(&id).cloned().context("not found"));
        let independent = identity_with(home.path(), vec![fork.clone()], read)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            independent,
            Identity {
                root: fork,
                child: false
            }
        );
    }

    #[test]
    fn official_bare_models_never_select_third_parties_or_an_ambiguous_second_account() {
        let mut gateway = gateway();
        let trusted = vec!["trusted-model".into()];
        let plan = plan_with_models(&gateway, "trusted-model", None, None, &trusted).unwrap();
        assert_eq!(plan.candidates.len(), 1);
        assert_eq!(plan.candidates[0].relay.id, "official");
        let explicit =
            plan_with_models(&gateway, "api/trusted-model", None, None, &trusted).unwrap();
        assert_eq!(
            explicit.candidates[0].relay.id, "api",
            "explicit API model selectors stay explicit"
        );
        gateway.providers.push(provider("official-two", true));
        assert!(plan_with_models(&gateway, "trusted-model", None, None, &trusted).is_err());
        gateway
            .providers
            .retain(|provider| provider.relay.id == "api");
        assert!(plan_with_models(&gateway, "trusted-model", None, None, &trusted).is_err());
    }

    #[test]
    fn child_sources_fail_closed_when_removed_disabled_or_model_is_missing() {
        let root = Identity {
            root: id(),
            child: false,
        };
        let child = Identity {
            root: root.root.clone(),
            child: true,
        };
        let generation = begin_root(&root).unwrap();
        remember_root(&root, generation, "api");
        let mut gateway = gateway();
        assert!(fixed_child_plan(&gateway, "unsupported-model", &child).is_err());
        gateway.providers[0].enabled = false;
        assert!(fixed_child_plan(&gateway, "trusted-model", &child).is_err());
        gateway.providers.remove(0);
        assert!(fixed_child_plan(&gateway, "trusted-model", &child).is_err());
    }

    #[test]
    fn ordinary_cli_keys_are_not_native_identity_proofs() {
        assert!(
            request_ids(ProxySessionHeaders {
                session_id: Some("cli-session"),
                ..Default::default()
            })
            .unwrap()
            .is_empty()
        );
        assert!(
            request_ids(ProxySessionHeaders {
                thread_id: Some("forged"),
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn root_compaction_uses_its_confirmed_source_without_reselecting_an_official_account() {
        let root = Identity {
            root: id(),
            child: false,
        };
        let child = Identity {
            root: root.root.clone(),
            child: true,
        };
        let gateway = gateway();
        assert!(
            continuation_plan(&gateway, "trusted-model", &root).is_err(),
            "missing source is never guessed"
        );
        let generation = begin_root(&root).unwrap();
        remember_root(&root, generation, "api");
        for identity in [&root, &child] {
            let plan = continuation_plan(&gateway, "trusted-model", identity).unwrap();
            assert!(!plan.candidates.is_empty());
            assert!(
                plan.candidates
                    .iter()
                    .all(|candidate| candidate.relay.id == "api")
            );
        }
        assert_eq!(
            remembered(&root).unwrap(),
            "api",
            "compaction never replaces the root binding"
        );
    }

    #[test]
    fn child_images_never_use_a_different_global_account_or_guess_after_restart() {
        let root = Identity {
            root: id(),
            child: false,
        };
        let child = Identity {
            root: root.root.clone(),
            child: true,
        };
        let mut gateway = gateway();
        assert!(child_image_target(&gateway, &child).is_err());
        let generation = begin_root(&root).unwrap();
        remember_root(&root, generation, "official");
        let target = child_image_target(&gateway, &child).unwrap().unwrap();
        assert_eq!(target.relay.id, "official");
        let generation = begin_root(&root).unwrap();
        remember_root(&root, generation, "api");
        assert!(child_image_target(&gateway, &child).is_err());
        let generation = begin_root(&root).unwrap();
        remember_root(&root, generation, "official");
        gateway.providers[1].enabled = false;
        assert!(child_image_target(&gateway, &child).is_err());
    }

    #[tokio::test]
    async fn conflicting_native_session_identity_is_rejected() {
        let home = tempfile::tempdir().unwrap();
        let thread = id();
        let metadata = Metadata {
            id: thread.clone(),
            session_id: Some(id()),
            model_provider: "custom".into(),
            ephemeral: true,
            parent_thread_id: None,
            forked_from_id: None,
            is_subagent: false,
        };
        assert!(
            identity_with(home.path(), vec![thread], |_| std::future::ready(Ok(
                metadata.clone()
            )))
            .await
            .is_err()
        );
    }
}
