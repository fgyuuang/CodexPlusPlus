//! 官方体验内全部新旧子代理使用父对话当前来源；只读父子关系，不改写线程或凭据。
use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

#[derive(Debug)]
struct MissingPersistedIdentity;

impl std::fmt::Display for MissingPersistedIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("线程关系尚未写入原生数据库，停止账号跟随；不会回退到登录账号")
    }
}

impl std::error::Error for MissingPersistedIdentity {}

/// 只接受同一原生 app-server 的 thread/read 元数据，不接受请求正文中的身份声明。
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeThreadMetadata {
    id: String,
    session_id: Option<String>,
    ephemeral: bool,
    model_provider: String,
    forked_from_id: Option<String>,
    parent_thread_id: Option<String>,
    #[serde(default)]
    is_subagent: bool,
}

fn request_identities(
    thread_id: Option<&str>,
    session_id: Option<&str>,
) -> anyhow::Result<HashSet<String>> {
    let mut identities = HashSet::new();
    for id in [thread_id, session_id].into_iter().flatten() {
        identities.insert(
            uuid::Uuid::parse_str(id.trim())
                .context("官方子代理请求缺少有效线程标识")?
                .to_string(),
        );
    }
    ensure!(
        !identities.is_empty(),
        "官方子代理跟随需要线程标识；不会回退到登录账号"
    );
    Ok(identities)
}

pub fn official_provider(provider: &str) -> bool {
    provider == "openai"
        || provider == crate::official_account_schedule::PROVIDER_ID
        || provider.starts_with(crate::official_experience::DIRECT_OFFICIAL_PROVIDER_PREFIX)
}

pub fn inherited_provider(
    home: &Path,
    thread_id: Option<&str>,
    session_id: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let identities = request_identities(thread_id, session_id)?;
    let mut selected = None;
    let mut known = HashSet::new();
    let mut roots = HashSet::new();
    for path in crate::codex_sqlite::codex_thread_reference_db_paths_from_home(home) {
        if !path.is_file() {
            continue;
        }
        let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.busy_timeout(std::time::Duration::from_millis(100))?;
        let has_threads: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='threads')",
            [],
            |r| r.get(0),
        )?;
        if !has_threads {
            continue;
        }
        for id in &identities {
            let source: Option<String> = db
                .query_row("SELECT source FROM threads WHERE id=?1", [id], |r| r.get(0))
                .optional()?;
            let Some(source) = source else {
                continue;
            };
            known.insert(id.clone());
            let provider = root_provider(&db, id, &source)?;
            if let Some(provider) = provider {
                ensure!(
                    selected.as_ref().is_none_or(|old| old == &provider),
                    "父子线程来源存在冲突，停止账号跟随"
                );
                selected = Some(provider);
            } else {
                roots.insert(id.clone());
            }
        }
    }
    if known.len() != identities.len() {
        return Err(MissingPersistedIdentity.into());
    }
    ensure!(roots.len() <= 1, "请求标识属于不同对话，停止账号跟随");
    if let Some((root, _)) = &selected {
        ensure!(
            roots.iter().all(|id| id == root),
            "请求标识属于不同对话，停止账号跟随"
        );
    }
    Ok(selected.map(|(_, provider)| provider))
}

/// 临时标题、描述和环境建议不写 threads；用原生内存状态证明身份，不能把缺行当作临时任务。
pub(crate) async fn inherited_provider_with_native_metadata(
    home: &Path,
    thread_id: Option<&str>,
    session_id: Option<&str>,
    incoming_provider: &str,
) -> anyhow::Result<Option<String>> {
    tokio::time::timeout(
        std::time::Duration::from_secs(6),
        inherited_provider_with_reader(
            home,
            thread_id,
            session_id,
            incoming_provider,
            read_native_thread_metadata,
        ),
    )
    .await
    .context("原生线程身份只读查询超时，停止账号跟随")?
}

async fn inherited_provider_with_reader<F, Fut>(
    home: &Path,
    thread_id: Option<&str>,
    session_id: Option<&str>,
    incoming_provider: &str,
    mut read: F,
) -> anyhow::Result<Option<String>>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<NativeThreadMetadata>>,
{
    match inherited_provider(home, thread_id, session_id) {
        Ok(provider) => return Ok(provider),
        Err(error) if error.downcast_ref::<MissingPersistedIdentity>().is_some() => {}
        Err(error) => return Err(error),
    }
    let identities = request_identities(thread_id, session_id)?;
    let mut cache = HashMap::<String, NativeThreadMetadata>::new();
    let mut selected = None;
    for id in identities {
        let mut current = id;
        let mut seen = HashSet::new();
        let mut first = true;
        let resolved = loop {
            ensure!(
                seen.len() < 64 && seen.insert(current.clone()),
                "原生线程谱系有环或超过限制，停止账号跟随"
            );
            if let Some(route) = persisted_identity_route(home, &current)? {
                break route;
            }
            let metadata = match cache.get(&current) {
                Some(value) => value.clone(),
                None => {
                    let value = read(current.clone()).await?;
                    cache.insert(current.clone(), value.clone());
                    value
                }
            };
            ensure!(
                metadata.id == current,
                "原生线程查询返回了其他身份，停止账号跟随"
            );
            if let Some(session) = &metadata.session_id {
                ensure!(
                    uuid::Uuid::parse_str(session).is_ok(),
                    "原生线程 session 标识无效"
                );
            }
            ensure!(
                official_provider(&metadata.model_provider),
                "原生线程已使用非官方来源，停止旧官方请求"
            );
            if first {
                ensure!(
                    metadata.model_provider == incoming_provider,
                    "原生线程来源与请求通道不一致，停止账号跟随"
                );
            }
            // 持久化 fork 是独立对话；只有 ephemeral fork 才把 forkedFromId 用作辅助任务谱系。
            let parent = metadata.parent_thread_id.as_ref().or_else(|| {
                metadata
                    .ephemeral
                    .then_some(metadata.forked_from_id.as_ref())
                    .flatten()
            });
            if let Some(parent) = parent {
                current = uuid::Uuid::parse_str(parent)
                    .context("原生线程父标识无效")?
                    .to_string();
                first = false;
                continue;
            }
            ensure!(
                !metadata.is_subagent,
                "原生子代理缺少父线程关系，停止账号跟随"
            );
            break (current, metadata.model_provider);
        };
        ensure!(
            selected.as_ref().is_none_or(|old| old == &resolved),
            "请求标识属于不同对话或来源，停止账号跟随"
        );
        selected = Some(resolved);
    }
    Ok(selected.map(|(_, provider)| provider))
}

fn persisted_identity_route(home: &Path, id: &str) -> anyhow::Result<Option<(String, String)>> {
    let mut selected = None;
    for path in crate::codex_sqlite::codex_thread_reference_db_paths_from_home(home) {
        if !path.is_file() {
            continue;
        }
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.busy_timeout(std::time::Duration::from_millis(100))?;
        let has_threads: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='threads')",
            [],
            |r| r.get(0),
        )?;
        if !has_threads {
            continue;
        }
        let row: Option<(String, String)> = db
            .query_row(
                "SELECT source,model_provider FROM threads WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((source, provider)) = row else {
            continue;
        };
        let route = root_provider(&db, id, &source)?.unwrap_or_else(|| (id.to_string(), provider));
        ensure!(
            official_provider(&route.1),
            "父对话已使用非官方来源，停止旧官方子代理请求"
        );
        ensure!(
            selected.as_ref().is_none_or(|old| old == &route),
            "父子线程来源存在冲突，停止账号跟随"
        );
        selected = Some(route);
    }
    Ok(selected)
}

async fn read_native_thread_metadata(id: String) -> anyhow::Result<NativeThreadMetadata> {
    let port = crate::status::StatusStore::default()
        .load_latest()?
        .and_then(|status| status.debug_port)
        .context("没有可验证的原生线程查询入口")?;
    read_native_thread_metadata_from_port(port, id).await
}

async fn read_native_thread_metadata_from_port(
    port: u16,
    id: String,
) -> anyhow::Result<NativeThreadMetadata> {
    let targets = crate::cdp::list_targets(port).await?;
    let target = crate::cdp::pick_injectable_codex_page_target(&targets)?;
    let websocket = target
        .web_socket_debugger_url
        .context("原生线程查询入口缺少调试地址")?;
    let script = format!(
        "(async()=>{{const read=window.__codexPlusReadNativeThreadMetadata;if(typeof read!==\"function\")throw new Error(\"native metadata reader unavailable\");return JSON.stringify(await read({}));}})()",
        serde_json::to_string(&id)?
    );
    let response =
        crate::bridge::evaluate_script_with_await_promise(&websocket, &script, true).await?;
    let metadata = response
        .pointer("/result/result/value")
        .and_then(serde_json::Value::as_str)
        .context("原生线程身份只读查询没有返回元数据")?;
    serde_json::from_str(metadata).context("原生线程身份元数据不完整")
}

fn root_provider(
    db: &Connection,
    child: &str,
    source: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let has_edges: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='thread_spawn_edges')", [], |r| r.get(0))?;
    if !has_edges {
        ensure!(
            !source.contains("subagent"),
            "子代理父子关系不可用，停止账号跟随"
        );
        return Ok(None);
    }
    let mut current = child.to_string();
    let mut seen = HashSet::new();
    for _ in 0..64 {
        ensure!(
            seen.insert(current.clone()),
            "子代理父子关系有环，停止账号跟随"
        );
        let parents = db
            .prepare(
                "SELECT DISTINCT parent_thread_id FROM thread_spawn_edges WHERE child_thread_id=?1",
            )?
            .query_map([&current], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ensure!(parents.len() <= 1, "子代理父线程不唯一，停止账号跟随");
        if let Some(parent) = parents.into_iter().next() {
            current = parent;
            continue;
        }
        if current == child {
            ensure!(
                !source.contains("subagent"),
                "子代理缺少父线程关系，停止账号跟随"
            );
            return Ok(None);
        }
        let provider: String = db
            .query_row(
                "SELECT model_provider FROM threads WHERE id=?1",
                [&current],
                |r| r.get(0),
            )
            .context("子代理父线程不可用，停止账号跟随")?;
        ensure!(
            official_provider(&provider),
            "父对话已使用非官方来源，停止旧官方子代理请求"
        );
        return Ok(Some((current, provider)));
    }
    anyhow::bail!("子代理层级超过限制，停止账号跟随")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_metadata_transport_uses_verified_codex_page_and_read_only_reader() {
        use futures_util::{SinkExt, StreamExt};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio_tungstenite::tungstenite::Message;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let id = uuid::Uuid::new_v4().to_string();
        let expected_id = id.clone();
        let server = tokio::spawn(async move {
            let (mut http, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let count = http.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
            }
            assert!(String::from_utf8_lossy(&request).starts_with("GET /json "));
            let body = serde_json::json!([{
                "id":"native-page","type":"page","title":"Codex","url":"app://-/index.html",
                "webSocketDebuggerUrl":format!("ws://127.0.0.1:{port}/devtools/page/native-page")
            }])
            .to_string();
            http.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            drop(http);
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let message = websocket.next().await.unwrap().unwrap();
            let request: serde_json::Value =
                serde_json::from_str(message.to_text().unwrap()).unwrap();
            assert_eq!(request["method"], "Runtime.evaluate");
            assert_eq!(request["params"]["awaitPromise"], true);
            let script = request["params"]["expression"].as_str().unwrap();
            assert!(script.contains("__codexPlusReadNativeThreadMetadata"));
            assert!(script.contains(&expected_id));
            for forbidden in [
                "turn/start",
                "thread/start",
                "thread/resume",
                "config",
                "prompt",
                "Authorization",
            ] {
                assert!(!script.contains(forbidden));
            }
            let metadata = serde_json::json!({"id":expected_id,"sessionId":expected_id,"ephemeral":true,"modelProvider":"openai","forkedFromId":null,"parentThreadId":null,"isSubagent":false});
            websocket.send(Message::Text(serde_json::json!({"id":request["id"],"result":{"result":{"type":"string","value":metadata.to_string()}}}).to_string().into())).await.unwrap();
        });
        let metadata = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            read_native_thread_metadata_from_port(port, id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(metadata.id, id);
        assert!(metadata.ephemeral);
        assert_eq!(metadata.model_provider, "openai");
        server.await.unwrap();
    }

    fn metadata(
        id: &str,
        provider: &str,
        ephemeral: bool,
        parent: Option<&str>,
    ) -> NativeThreadMetadata {
        NativeThreadMetadata {
            id: id.into(),
            session_id: Some(id.into()),
            ephemeral,
            model_provider: provider.into(),
            forked_from_id: parent.map(str::to_string),
            parent_thread_id: None,
            is_subagent: false,
        }
    }

    async fn resolve_metadata(
        home: &Path,
        thread: &str,
        session: Option<&str>,
        snapshots: &[NativeThreadMetadata],
    ) -> anyhow::Result<Option<String>> {
        let snapshots: HashMap<_, _> = snapshots
            .iter()
            .cloned()
            .map(|value| (value.id.clone(), value))
            .collect();
        inherited_provider_with_reader(home, Some(thread), session, "openai", move |id| {
            std::future::ready(
                snapshots
                    .get(&id)
                    .cloned()
                    .context("native thread not found"),
            )
        })
        .await
    }

    #[tokio::test]
    async fn native_metadata_proves_unpersisted_auxiliary_tasks_without_relaxing_unknown_requests()
    {
        let _guard = crate::paths::settings_path_test_guard();
        let home = tempfile::tempdir().unwrap();
        let title = uuid::Uuid::new_v4().to_string();
        let unknown = uuid::Uuid::new_v4().to_string();
        // 标题、description、环境建议都以原生 ephemeral 身份证明，而非模型名或提示词分类。
        assert_eq!(
            resolve_metadata(
                home.path(),
                &title,
                Some(&title),
                &[metadata(&title, "openai", true, None)]
            )
            .await
            .unwrap()
            .as_deref(),
            Some("openai")
        );
        assert!(!home.path().join("state_5.sqlite").exists());
        assert!(
            resolve_metadata(home.path(), &unknown, None, &[])
                .await
                .is_err()
        );
        assert!(
            resolve_metadata(
                home.path(),
                &title,
                Some(&unknown),
                &[metadata(&title, "openai", true, None)]
            )
            .await
            .is_err()
        );
        assert!(
            resolve_metadata(
                home.path(),
                &title,
                None,
                &[metadata(&title, "codex_plus_official_account2", true, None)]
            )
            .await
            .is_err()
        );
        let forged = metadata(&unknown, "openai", true, None);
        assert!(
            inherited_provider_with_reader(home.path(), Some(&title), None, "openai", |_| {
                std::future::ready(Ok(forged.clone()))
            })
            .await
            .is_err()
        );
        let mut malformed = metadata(&title, "openai", true, None);
        malformed.session_id = Some("not-a-session".into());
        assert!(
            resolve_metadata(home.path(), &title, None, &[malformed])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn auxiliary_forks_follow_verified_current_ancestry_and_keep_independent_threads_separate()
     {
        let _guard = crate::paths::settings_path_test_guard();
        let home = tempfile::tempdir().unwrap();
        let root = uuid::Uuid::new_v4().to_string();
        let helper = uuid::Uuid::new_v4().to_string();
        let nested = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        let db = Connection::open(home.path().join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,source TEXT,model_provider TEXT); CREATE TABLE thread_spawn_edges(parent_thread_id TEXT,child_thread_id TEXT);").unwrap();
        db.execute(
            "INSERT INTO threads VALUES(?1,'cli','codex_plus_official_account2')",
            [&root],
        )
        .unwrap();
        db.execute("INSERT INTO threads VALUES(?1,'cli','openai')", [&other])
            .unwrap();
        let first = metadata(&helper, "openai", true, Some(&root));
        let second = metadata(&nested, "openai", true, Some(&helper));
        assert_eq!(
            resolve_metadata(
                home.path(),
                &nested,
                Some(&root),
                &[first.clone(), second.clone()]
            )
            .await
            .unwrap()
            .as_deref(),
            Some("codex_plus_official_account2")
        );
        assert!(
            resolve_metadata(
                home.path(),
                &nested,
                Some(&other),
                &[first.clone(), second.clone()]
            )
            .await
            .is_err()
        );
        db.execute(
            "UPDATE threads SET model_provider='codex_plus_official_schedule' WHERE id=?1",
            [&root],
        )
        .unwrap();
        assert_eq!(
            resolve_metadata(home.path(), &helper, None, &[first.clone()])
                .await
                .unwrap()
                .as_deref(),
            Some("codex_plus_official_schedule")
        );
        // 普通新建线程写库竞态也需原生证明；持久化 fork 仍是独立对话，不能借 forkedFromId 跟随。
        assert_eq!(
            resolve_metadata(
                home.path(),
                &helper,
                None,
                &[metadata(&helper, "openai", false, Some(&root))]
            )
            .await
            .unwrap()
            .as_deref(),
            Some("openai")
        );
        let mut child_without_parent = metadata(&helper, "openai", false, None);
        child_without_parent.is_subagent = true;
        assert!(
            resolve_metadata(home.path(), &helper, None, &[child_without_parent])
                .await
                .is_err()
        );
        let cycle = metadata(&helper, "openai", true, Some(&nested));
        assert!(
            resolve_metadata(home.path(), &helper, None, &[cycle, second])
                .await
                .is_err()
        );
        assert!(
            resolve_metadata(
                home.path(),
                &helper,
                None,
                &[metadata(&helper, "custom", true, None)]
            )
            .await
            .is_err()
        );
        db.execute(
            "UPDATE threads SET model_provider='custom' WHERE id=?1",
            [&root],
        )
        .unwrap();
        assert!(
            resolve_metadata(home.path(), &helper, None, &[first])
                .await
                .is_err()
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM threads", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn persisted_conflicts_never_enter_native_fallback_and_unrelated_roots_are_rejected() {
        let _guard = crate::paths::settings_path_test_guard();
        let home = tempfile::tempdir().unwrap();
        let child = uuid::Uuid::new_v4().to_string();
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        let db = Connection::open(home.path().join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,source TEXT,model_provider TEXT); CREATE TABLE thread_spawn_edges(parent_thread_id TEXT,child_thread_id TEXT);").unwrap();
        for (id, source) in [(&child, "subagent"), (&first, "cli"), (&second, "cli")] {
            db.execute(
                "INSERT INTO threads VALUES(?1,?2,'openai')",
                [id.as_str(), source],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO thread_spawn_edges VALUES(?1,?2)",
            [&first, &child],
        )
        .unwrap();
        db.execute(
            "INSERT INTO thread_spawn_edges VALUES(?1,?2)",
            [&second, &child],
        )
        .unwrap();
        let calls = std::cell::Cell::new(0);
        assert!(
            inherited_provider_with_reader(home.path(), Some(&child), None, "openai", |_| {
                calls.set(calls.get() + 1);
                std::future::ready(Err(anyhow::anyhow!("must not query")))
            })
            .await
            .is_err()
        );
        assert_eq!(calls.get(), 0);
        assert!(inherited_provider(home.path(), Some(&first), Some(&second)).is_err());
    }

    #[test]
    fn lookup_validates_identity_and_rejects_conflicting_families_without_writes() {
        let _guard = crate::paths::settings_path_test_guard();
        let home = tempfile::tempdir().unwrap();
        let root = uuid::Uuid::new_v4().to_string();
        let child = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        let db = Connection::open(home.path().join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,source TEXT,model_provider TEXT); CREATE TABLE thread_spawn_edges(parent_thread_id TEXT,child_thread_id TEXT);").unwrap();
        for (id, source, provider) in [
            (&root, "cli", "codex_plus_official_account2"),
            (&child, "subagent", "openai"),
            (&other, "cli", "openai"),
        ] {
            db.execute(
                "INSERT INTO threads VALUES(?1,?2,?3)",
                [id.as_str(), source, provider],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO thread_spawn_edges VALUES(?1,?2)",
            [&root, &child],
        )
        .unwrap();
        assert_eq!(
            inherited_provider(home.path(), Some(&child), Some(&root))
                .unwrap()
                .as_deref(),
            Some("codex_plus_official_account2")
        );
        assert!(
            inherited_provider(home.path(), Some(&root), Some(&root))
                .unwrap()
                .is_none()
        );
        assert!(inherited_provider(home.path(), Some(&child), Some(&other)).is_err());
        assert!(inherited_provider(home.path(), Some("invalid"), None).is_err());
        assert!(inherited_provider(home.path(), None, None).is_err());
        assert!(
            inherited_provider(home.path(), Some(&uuid::Uuid::new_v4().to_string()), None).is_err()
        );
        assert_eq!(
            db.query_row(
                "SELECT model_provider FROM threads WHERE id=?1",
                [&child],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "openai"
        );
        db.execute(
            "INSERT INTO thread_spawn_edges VALUES(?1,?2)",
            [&other, &child],
        )
        .unwrap();
        assert!(inherited_provider(home.path(), Some(&child), None).is_err());
    }

    #[test]
    fn existing_child_follows_latest_root_without_rewriting_child_or_other_roots() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,source TEXT,model_provider TEXT); CREATE TABLE thread_spawn_edges(parent_thread_id TEXT,child_thread_id TEXT); INSERT INTO threads VALUES('root','cli','openai'),('child','subagent','openai'),('nested','subagent','openai'),('other','cli','openai'); INSERT INTO thread_spawn_edges VALUES('root','child'),('child','nested');").unwrap();
        assert_eq!(
            root_provider(&db, "nested", "subagent").unwrap(),
            Some(("root".into(), "openai".into()))
        );
        db.execute(
            "UPDATE threads SET model_provider='codex_plus_official_account2' WHERE id='root'",
            [],
        )
        .unwrap();
        assert_eq!(
            root_provider(&db, "nested", "subagent").unwrap(),
            Some(("root".into(), "codex_plus_official_account2".into()))
        );
        assert!(root_provider(&db, "other", "cli").unwrap().is_none());
        db.execute(
            "INSERT INTO threads VALUES('new-child','subagent','openai')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO thread_spawn_edges VALUES('root','new-child')",
            [],
        )
        .unwrap();
        for child in ["child", "nested", "new-child"] {
            assert_eq!(
                root_provider(&db, child, "subagent").unwrap(),
                Some(("root".into(), "codex_plus_official_account2".into()))
            );
        }
        db.execute(
            "UPDATE threads SET model_provider='codex_plus_official_schedule' WHERE id='root'",
            [],
        )
        .unwrap();
        for child in ["child", "nested", "new-child"] {
            assert_eq!(
                root_provider(&db, child, "subagent").unwrap(),
                Some(("root".into(), "codex_plus_official_schedule".into()))
            );
        }
        assert!(root_provider(&db, "other", "cli").unwrap().is_none());
        assert_eq!(
            db.query_row(
                "SELECT model_provider FROM threads WHERE id='child'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "openai"
        );
        db.execute(
            "UPDATE threads SET model_provider='custom' WHERE id='root'",
            [],
        )
        .unwrap();
        assert!(root_provider(&db, "child", "subagent").is_err());
        assert!(root_provider(&db, "missing", "subagent").is_err());
        db.execute("INSERT INTO thread_spawn_edges VALUES('nested','root')", [])
            .unwrap();
        assert!(root_provider(&db, "child", "subagent").is_err());
    }
}
