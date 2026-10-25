//! 可选的官方子代理账号跟随；只读原生父子关系，不改写线程或凭据。
use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

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
    let mut selected = None;
    let mut known = false;
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
            known = true;
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
    ensure!(
        known,
        "线程关系尚未写入原生数据库，停止账号跟随；不会回退到登录账号"
    );
    if let Some((root, _)) = &selected {
        ensure!(
            roots.iter().all(|id| id == root),
            "请求标识属于不同对话，停止账号跟随"
        );
    }
    Ok(selected.map(|(_, provider)| provider))
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
