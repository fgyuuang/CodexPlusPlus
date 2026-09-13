//! 官方 Codex 模型目录。
//!
//! 官方模型不是由 Codex++ 维护的一组固定名称。目录优先从当前 ChatGPT 登录账号
//! 的 `/backend-api/codex/models` 获取，并按账号缓存；没有网络时只使用已经缓存的
//! 官方条目、Codex bundled catalog 和仓库内置兼容条目。

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::timeout;

use crate::official_accounts::{OfficialAccountStore, auth_token, parse_official_auth};
use crate::settings::{BackendSettings, SettingsStore};

const CACHE_VERSION: u32 = 1;
const CACHE_STALE_AFTER_SECS: i64 = 24 * 60 * 60;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(4);
const OFFICIAL_MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";
const BUNDLED_CATALOG_JSON: &str = include_str!("../assets/codex-models.json");
const GPT56_COMPAT_CATALOG_JSON: &str =
    include_str!("../../../assets/gpt56-model-metadata-compat.json");
const FUTURE_COMPAT_CATALOG_JSON: &str =
    include_str!("../../../assets/astra-model-metadata-compat.json");

static BUNDLED_CATALOG_CACHE: OnceLock<Mutex<BTreeMap<String, Option<Vec<Value>>>>> =
    OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialModelCatalogStatus {
    pub status: String,
    pub source: String,
    pub visible_models: Vec<String>,
    pub fetched_at: Option<i64>,
    pub client_version: String,
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedOfficialCatalog {
    account_id: String,
    fetched_at: i64,
    client_version: String,
    #[serde(default)]
    etag: Option<String>,
    #[serde(default)]
    source: String,
    models: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CatalogCacheFile {
    version: u32,
    #[serde(default)]
    accounts: BTreeMap<String, CachedOfficialCatalog>,
}

impl Default for CatalogCacheFile {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            accounts: BTreeMap::new(),
        }
    }
}

#[derive(Debug)]
enum CatalogRequestError {
    Unauthorized,
    Other(String),
}

impl std::fmt::Display for CatalogRequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => formatter.write_str("HTTP 401"),
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for CatalogRequestError {}

pub fn official_model_catalog_path() -> PathBuf {
    crate::paths::default_official_model_catalog_path()
}

/// 返回当前设置对应的安全目录状态，不读取任何认证密钥。
pub fn status_for_current_settings() -> OfficialModelCatalogStatus {
    let settings = SettingsStore::default().load().unwrap_or_default();
    status_for_settings(&settings, None)
}

pub fn status_for_settings(
    settings: &BackendSettings,
    error: Option<String>,
) -> OfficialModelCatalogStatus {
    let account_id = settings.active_official_account_id.trim();
    let (entries, source, fetched_at, client_version) =
        fallback_entries_for_settings(settings, account_id);
    let models = visible_slugs(&entries);
    let stale = fetched_at.is_some_and(|fetched_at| fetched_at + CACHE_STALE_AFTER_SECS < now_ts());
    let status = if models.is_empty() {
        "not_configured"
    } else if stale {
        "stale"
    } else {
        "ok"
    };
    OfficialModelCatalogStatus {
        status: status.to_string(),
        source,
        visible_models: models,
        fetched_at,
        client_version,
        stale,
        error,
    }
}

/// 刷新当前活动官方账号。刷新失败由调用方决定是否提示；该函数本身不会把错误
/// 写入缓存，也不会阻止 Codex 启动。
pub async fn refresh_active_account(force: bool) -> anyhow::Result<OfficialModelCatalogStatus> {
    let settings = SettingsStore::default().load().unwrap_or_default();
    let account_id = settings.active_official_account_id.trim().to_string();
    ensure!(!account_id.is_empty(), "没有活动官方账号");
    refresh_account(&account_id, &settings, force).await
}

pub async fn refresh_account(
    account_id: &str,
    settings: &BackendSettings,
    force: bool,
) -> anyhow::Result<OfficialModelCatalogStatus> {
    let account_id = account_id.trim();
    ensure!(!account_id.is_empty(), "官方账号 ID 不能为空");
    let store = OfficialAccountStore::default();
    let mut auth = store.get_auth_json(account_id)?;
    let parsed = parse_official_auth(&auth)?;
    let client_version = discover_client_version(settings).await;
    let previous = load_cache()
        .ok()
        .and_then(|cache| cache.accounts.get(account_id).cloned());
    let previous_etag = cached_etag_for_request(previous.as_ref(), &client_version, force);

    let mut response = request_catalog(
        &auth,
        &parsed.chatgpt_account_id,
        &client_version,
        previous_etag,
    )
    .await;
    if matches!(response, Err(CatalogRequestError::Unauthorized)) {
        // 令牌过期时只刷新一次，避免网络错误导致无限刷新循环。
        auth = store
            .refresh_tokens(account_id, true)
            .await
            .with_context(|| "官方模型目录请求返回 401，刷新令牌失败")
            .and_then(|_| store.get_auth_json(account_id))?;
        let parsed = parse_official_auth(&auth)?;
        response = request_catalog(
            &auth,
            &parsed.chatgpt_account_id,
            &client_version,
            previous_etag,
        )
        .await;
    }

    let snapshot = match response {
        Ok(CatalogResponse::NotModified) => {
            let mut previous = previous.context("官方返回 304，但本地没有可用目录快照")?;
            previous.fetched_at = now_ts();
            previous.client_version = client_version.clone();
            previous.source = "account_snapshot".to_string();
            previous
        }
        Ok(CatalogResponse::Models { models, etag }) => CachedOfficialCatalog {
            account_id: account_id.to_string(),
            fetched_at: now_ts(),
            client_version: client_version.clone(),
            etag,
            source: "account_snapshot".to_string(),
            models,
        },
        Err(error) => {
            let safe_error = sanitize_error(&error.to_string());
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "official_model_catalog.refresh_failed",
                json!({
                    "accountId": account_id,
                    "clientVersion": client_version,
                    "error": safe_error,
                }),
            );
            // 目录刷新是增强能力，回退结果仍然交给 Codex 使用。
            return Ok(status_for_settings(settings, Some(safe_error)));
        }
    };

    let mut cache = load_cache().unwrap_or_default();
    cache.version = CACHE_VERSION;
    cache.accounts.insert(account_id.to_string(), snapshot);
    save_cache(&cache)?;
    let status = status_for_settings(settings, None);
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "official_model_catalog.refreshed",
        json!({
            "accountId": account_id,
            "source": status.source,
            "models": status.visible_models.len(),
            "clientVersion": status.client_version,
            "stale": status.stale,
        }),
    );
    Ok(status)
}

/// 当前活动账号可用于路由判定的官方 slug。隐藏模型也属于可信来源，但不会出现在
/// 选择器中；因此路由和 UI 分别使用这两个接口。
pub fn trusted_official_model_slugs() -> Vec<String> {
    if let Ok(settings) = SettingsStore::default().load() {
        return trusted_official_model_slugs_for_settings(&settings);
    }
    unique_slugs(&compatibility_entries(true))
}

pub fn trusted_official_model_slugs_for_settings(settings: &BackendSettings) -> Vec<String> {
    let account_id = settings.active_official_account_id.trim();
    if !account_id.is_empty()
        && let Ok(cache) = load_cache()
        && let Some(snapshot) = cache.accounts.get(account_id)
    {
        return unique_slugs(&snapshot.models);
    }
    let (entries, _, _, _) = fallback_entries_for_settings(settings, account_id);
    unique_slugs(&entries)
}

pub fn visible_official_model_slugs() -> Vec<String> {
    if let Ok(settings) = SettingsStore::default().load() {
        return visible_official_model_slugs_for_settings(&settings);
    }
    let settings = BackendSettings::default();
    let (entries, _, _, _) = fallback_entries_for_settings(&settings, "");
    visible_slugs(&entries)
}

pub fn visible_official_model_slugs_for_settings(settings: &BackendSettings) -> Vec<String> {
    let account_id = settings.active_official_account_id.trim();
    if !account_id.is_empty()
        && let Ok(cache) = load_cache()
        && let Some(snapshot) = cache.accounts.get(account_id)
    {
        return visible_slugs(&snapshot.models);
    }
    let (entries, _, _, _) = fallback_entries_for_settings(settings, account_id);
    visible_slugs(&entries)
}

pub fn is_trusted_official_model(model: &str) -> bool {
    let normalized = model.trim();
    trusted_official_model_slugs()
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(normalized))
}

pub fn official_model_metadata(slug: &str) -> Option<Value> {
    if let Ok(settings) = SettingsStore::default().load() {
        return official_model_metadata_for_settings(&settings, slug);
    }
    find_entry(&compatibility_entries(true), slug)
}

pub fn official_model_metadata_for_settings(
    settings: &BackendSettings,
    slug: &str,
) -> Option<Value> {
    let slug = slug.trim();
    if slug.is_empty() {
        return None;
    }
    let account_id = settings.active_official_account_id.trim();
    if !account_id.is_empty()
        && let Ok(cache) = load_cache()
        && let Some(snapshot) = cache.accounts.get(account_id)
        && let Some(entry) = find_entry(&snapshot.models, slug)
    {
        return Some(entry);
    }
    find_entry(&official_model_entries_for_settings(settings), slug)
}

pub fn official_model_catalog_value(settings: &BackendSettings) -> Value {
    json!({ "models": official_model_entries_for_settings(settings) })
}

/// 返回当前活动账号的官方条目；没有快照时回退到仓库内置兼容目录。
pub fn official_model_entries_for_settings(settings: &BackendSettings) -> Vec<Value> {
    let account_id = settings.active_official_account_id.trim();
    if let Some(entries) = cached_official_model_entries_for_account(account_id) {
        return entries;
    }
    fallback_entries_for_settings(settings, account_id).0
}

pub fn official_model_entries_for_current_settings() -> Vec<Value> {
    SettingsStore::default()
        .load()
        .map(|settings| official_model_entries_for_settings(&settings))
        .unwrap_or_else(|_| compatibility_entries(true))
}

/// 返回当前活动账号已下载的官方条目，不读取其它账号快照。
pub fn cached_official_model_entries() -> Vec<Value> {
    let Ok(settings) = SettingsStore::default().load() else {
        return Vec::new();
    };
    cached_official_model_entries_for_settings(&settings)
}

pub fn cached_official_model_entries_for_settings(settings: &BackendSettings) -> Vec<Value> {
    cached_official_model_entries_for_account(settings.active_official_account_id.trim())
        .unwrap_or_default()
}

fn cached_official_model_entries_for_account(account_id: &str) -> Option<Vec<Value>> {
    if account_id.trim().is_empty() {
        return None;
    }
    let cache = load_cache().ok()?;
    let snapshot = cache.accounts.get(account_id.trim())?;
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for entry in &snapshot.models {
        let Some(slug) = entry.get("slug").and_then(Value::as_str) else {
            continue;
        };
        if seen.insert(slug.to_ascii_lowercase()) {
            entries.push(entry.clone());
        }
    }
    Some(entries)
}

fn fallback_entries_for_settings(
    settings: &BackendSettings,
    account_id: &str,
) -> (Vec<Value>, String, Option<i64>, String) {
    if let Some(snapshot) = (!account_id.is_empty())
        .then(|| {
            load_cache()
                .ok()
                .and_then(|cache| cache.accounts.get(account_id).cloned())
        })
        .flatten()
    {
        return (
            snapshot.models,
            if snapshot.source.trim().is_empty() {
                "account_snapshot".to_string()
            } else {
                snapshot.source
            },
            Some(snapshot.fetched_at),
            snapshot.client_version,
        );
    }

    if let Some(entries) = bundled_cli_catalog(settings) {
        let entries = merge_catalog_entries(entries, compatibility_entries(true));
        return (
            entries,
            "bundled_cli+compatibility".to_string(),
            None,
            discover_client_version_sync(settings),
        );
    }

    (
        compatibility_entries(true),
        "compatibility".to_string(),
        None,
        discover_client_version_sync(settings),
    )
}

fn bundled_cli_catalog(settings: &BackendSettings) -> Option<Vec<Value>> {
    let key = codex_cli_candidates(settings)
        .iter()
        .map(|candidate| candidate.to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("\n");
    let cache = BUNDLED_CATALOG_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Ok(mut cache) = cache.lock() {
        if let Some(result) = cache.get(&key) {
            return result.clone();
        }
        let result = bundled_cli_catalog_sync(settings);
        cache.insert(key, result.clone());
        return result;
    }
    bundled_cli_catalog_sync(settings)
}

fn bundled_cli_catalog_sync(settings: &BackendSettings) -> Option<Vec<Value>> {
    for candidate in codex_cli_candidates(settings) {
        let Some(output) = run_bundled_catalog_command(&candidate) else {
            continue;
        };
        let entries = parse_catalog_output(&output);
        if !entries.is_empty() {
            return Some(entries);
        }
    }
    None
}

fn run_bundled_catalog_command(candidate: &Path) -> Option<Vec<u8>> {
    let mut child = StdCommand::new(candidate)
        .args(["debug", "models", "--bundled"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + COMMAND_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(_) => return None,
        }
    }
    child.wait_with_output().ok().and_then(|output| {
        output
            .status
            .success()
            .then_some(output.stdout)
            .filter(|bytes| !bytes.is_empty())
    })
}

fn parse_catalog_output(bytes: &[u8]) -> Vec<Value> {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if let Ok(payload) = serde_json::from_str::<Value>(trimmed) {
        return parse_model_entries(&payload);
    }
    for (open, close) in [('{', '}'), ('[', ']')] {
        let Some(start) = trimmed.find(open) else {
            continue;
        };
        let Some(end) = trimmed.rfind(close) else {
            continue;
        };
        if end <= start {
            continue;
        }
        if let Ok(payload) = serde_json::from_str::<Value>(&trimmed[start..=end]) {
            let entries = parse_model_entries(&payload);
            if !entries.is_empty() {
                return entries;
            }
        }
    }
    Vec::new()
}

fn codex_cli_candidates(settings: &BackendSettings) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(app_dir) = crate::app_paths::resolve_codex_app_dir_with_saved(
        None,
        Some(settings.codex_app_path.as_str()),
    ) {
        if let Some(cli) = crate::app_paths::find_bundled_codex_cli(&app_dir) {
            candidates.push(cli);
        }
        candidates.push(crate::app_paths::build_codex_executable(&app_dir));
    }
    if let Some(cli_dir) = crate::app_paths::find_standalone_codex_app_dir() {
        candidates.push(crate::app_paths::build_codex_executable(&cli_dir));
    }
    #[cfg(windows)]
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        candidates.extend(windows_local_codex_cli_candidates(Path::new(
            &local_app_data,
        )));
    }
    candidates.push(PathBuf::from("codex"));
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.to_string_lossy().to_ascii_lowercase()))
        .collect()
}

fn cached_etag_for_request<'a>(
    previous: Option<&'a CachedOfficialCatalog>,
    client_version: &str,
    force: bool,
) -> Option<&'a str> {
    if force {
        return None;
    }
    previous
        .filter(|snapshot| snapshot.client_version == client_version)
        .and_then(|snapshot| snapshot.etag.as_deref())
        .filter(|etag| !etag.trim().is_empty())
}

#[cfg(windows)]
fn windows_local_codex_cli_candidates(local_app_data: &Path) -> Vec<PathBuf> {
    let bin_dir = local_app_data.join("OpenAI").join("Codex").join("bin");
    let Ok(entries) = std::fs::read_dir(&bin_dir) else {
        return Vec::new();
    };
    let mut candidates = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("codex.exe"))
        .filter(|candidate| candidate.is_file())
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        let left_modified = left
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok();
        let right_modified = right
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok();
        right_modified
            .cmp(&left_modified)
            .then_with(|| right.cmp(left))
    });
    candidates
}

async fn request_catalog(
    auth: &Value,
    account_id: &str,
    client_version: &str,
    etag: Option<&str>,
) -> Result<CatalogResponse, CatalogRequestError> {
    let access_token = auth_token(auth, "access_token")
        .ok_or_else(|| CatalogRequestError::Other("账号缺少 access_token".to_string()))?;
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(format!("codex_cli_rs/{client_version}"))
        .build()
        .map_err(|_| CatalogRequestError::Other("创建官方目录客户端失败".to_string()))?;
    let endpoint = std::env::var("CODEX_PLUS_OFFICIAL_MODEL_CATALOG_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| OFFICIAL_MODELS_URL.to_string());
    let mut request = client
        .get(endpoint)
        .query(&[("client_version", client_version)])
        .bearer_auth(access_token)
        .header("ChatGPT-Account-ID", account_id)
        .header("originator", "codex_cli_rs")
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(etag) = etag.filter(|value| !value.trim().is_empty()) {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let response = request
        .send()
        .await
        .map_err(|error| CatalogRequestError::Other(sanitize_error(&error.to_string())))?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(CatalogResponse::NotModified);
    }
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(CatalogRequestError::Unauthorized);
    }
    let status = response.status();
    if !status.is_success() {
        return Err(CatalogRequestError::Other(format!(
            "HTTP {}",
            status.as_u16()
        )));
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let payload = response.json::<Value>().await.map_err(|error| {
        CatalogRequestError::Other(format!(
            "官方目录 JSON 无效：{}",
            sanitize_error(&error.to_string())
        ))
    })?;
    let models = parse_model_entries(&payload);
    if models.is_empty() {
        return Err(CatalogRequestError::Other(
            "官方目录没有可用模型条目".to_string(),
        ));
    }
    Ok(CatalogResponse::Models { models, etag })
}

enum CatalogResponse {
    NotModified,
    Models {
        models: Vec<Value>,
        etag: Option<String>,
    },
}

fn parse_model_entries(payload: &Value) -> Vec<Value> {
    let value = if let Some(array) = payload.as_array() {
        Some(array)
    } else {
        payload
            .get("models")
            .and_then(Value::as_array)
            .or_else(|| payload.get("data").and_then(Value::as_array))
            .or_else(|| payload.get("items").and_then(Value::as_array))
    };
    let Some(items) = value else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    items
        .iter()
        .filter_map(|item| {
            let mut entry = match item {
                Value::String(slug) => json!({ "slug": slug.trim(), "display_name": slug.trim() }),
                Value::Object(_) => item.clone(),
                _ => return None,
            };
            let slug = entry
                .get("slug")
                .and_then(Value::as_str)
                .or_else(|| entry.get("id").and_then(Value::as_str))
                .or_else(|| entry.get("model").and_then(Value::as_str))
                .map(str::trim)
                .filter(|slug| !slug.is_empty())?
                .to_string();
            entry["slug"] = Value::String(slug.clone());
            if entry.get("display_name").is_none() {
                entry["display_name"] = Value::String(slug.clone());
            }
            seen.insert(slug.to_ascii_lowercase()).then_some(entry)
        })
        .collect()
}

fn load_cache() -> anyhow::Result<CatalogCacheFile> {
    let path = official_model_catalog_path();
    match std::fs::read(&path) {
        Ok(bytes) => {
            let cache: CatalogCacheFile = serde_json::from_slice(&bytes)
                .with_context(|| format!("官方模型目录缓存格式无效：{}", path.display()))?;
            Ok(cache)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(CatalogCacheFile::default())
        }
        Err(error) => Err(error.into()),
    }
}

fn save_cache(cache: &CatalogCacheFile) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(cache)?;
    crate::settings::atomic_write(&official_model_catalog_path(), &bytes)
}

fn find_entry(entries: &[Value], slug: &str) -> Option<Value> {
    entries.iter().find_map(|entry| {
        entry
            .get("slug")
            .and_then(Value::as_str)
            .filter(|candidate| candidate.eq_ignore_ascii_case(slug))
            .map(|_| entry.clone())
    })
}

fn unique_slugs(entries: &[Value]) -> Vec<String> {
    let mut seen = HashSet::new();
    entries
        .iter()
        .filter_map(|entry| entry.get("slug").and_then(Value::as_str))
        .map(str::trim)
        .filter(|slug| !slug.is_empty())
        .filter(|slug| seen.insert(slug.to_ascii_lowercase()))
        .map(ToString::to_string)
        .collect()
}

fn visible_slugs(entries: &[Value]) -> Vec<String> {
    let filtered = entries
        .iter()
        .filter(|entry| {
            entry
                .get("supported_in_api")
                .and_then(Value::as_bool)
                .unwrap_or(true)
        })
        .filter(|entry| {
            entry
                .get("visibility")
                .and_then(Value::as_str)
                .map(|value| value.eq_ignore_ascii_case("list"))
                .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();
    unique_slugs(&filtered)
}

fn bundled_catalog_entries() -> Vec<Value> {
    catalog_models(BUNDLED_CATALOG_JSON)
}

fn compatibility_entries(include_future: bool) -> Vec<Value> {
    let mut entries = bundled_catalog_entries();
    // Compatibility metadata is data, not a Rust model list. It is only used after
    // account snapshots and the actual Codex bundled catalog are unavailable.
    entries.extend(catalog_models(GPT56_COMPAT_CATALOG_JSON));
    if include_future {
        entries.extend(catalog_models(FUTURE_COMPAT_CATALOG_JSON));
    }
    let mut seen = HashSet::new();
    entries
        .into_iter()
        .filter(|entry| {
            entry
                .get("slug")
                .and_then(Value::as_str)
                .is_some_and(|slug| seen.insert(slug.to_ascii_lowercase()))
        })
        .collect()
}

fn merge_catalog_entries(primary: Vec<Value>, fallback: Vec<Value>) -> Vec<Value> {
    let mut entries = Vec::with_capacity(primary.len() + fallback.len());
    let mut seen = HashSet::new();
    for entry in primary.into_iter().chain(fallback) {
        let Some(slug) = entry.get("slug").and_then(Value::as_str) else {
            continue;
        };
        if seen.insert(slug.to_ascii_lowercase()) {
            entries.push(entry);
        }
    }
    entries
}

fn catalog_models(contents: &str) -> Vec<Value> {
    serde_json::from_str::<Value>(contents)
        .ok()
        .map(|value| parse_model_entries(&value))
        .unwrap_or_default()
}

async fn discover_client_version(settings: &BackendSettings) -> String {
    let mut versions = Vec::new();
    for candidate in codex_cli_candidates(settings) {
        if let Some(version) = command_version(&candidate).await {
            versions.push(version);
        }
    }
    if let Some(version) = cached_codex_client_version() {
        versions.push(version);
    }
    latest_client_version(versions).unwrap_or_else(|| "unknown".to_string())
}

fn discover_client_version_sync(settings: &BackendSettings) -> String {
    let mut versions = Vec::new();
    for candidate in codex_cli_candidates(settings) {
        if let Some(version) = command_version_sync(&candidate) {
            versions.push(version);
        }
    }
    if let Some(version) = cached_codex_client_version() {
        versions.push(version);
    }
    latest_client_version(versions).unwrap_or_else(|| "unknown".to_string())
}

async fn command_version(candidate: &Path) -> Option<String> {
    let candidate_text = candidate.to_string_lossy();
    if let Some(version) = candidate_text.strip_prefix("__version__:") {
        return normalize_client_version(version);
    }
    let output = timeout(
        COMMAND_TIMEOUT,
        Command::new(candidate)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_codex_cli_version_output(&text)
}

fn command_version_sync(candidate: &Path) -> Option<String> {
    let candidate_text = candidate.to_string_lossy();
    if let Some(version) = candidate_text.strip_prefix("__version__:") {
        return normalize_client_version(version);
    }
    let mut child = StdCommand::new(candidate)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + COMMAND_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(_) => return None,
        }
    }
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_codex_cli_version_output(&text)
}

fn parse_codex_cli_version_output(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let command = fields.next()?;
        if !command.eq_ignore_ascii_case("codex-cli") {
            return None;
        }
        normalize_client_version(fields.next()?)
    })
}

fn normalize_client_version(value: &str) -> Option<String> {
    let core = value
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+'])
        .next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(format!("{major}.{minor}.{patch}"))
}

fn latest_client_version(versions: Vec<String>) -> Option<String> {
    versions
        .into_iter()
        .filter_map(|version| {
            let normalized = normalize_client_version(&version)?;
            let parts = normalized
                .split('.')
                .map(str::parse::<u64>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            Some(((parts[0], parts[1], parts[2]), normalized))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, version)| version)
}

fn cached_codex_client_version() -> Option<String> {
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".codex")))?;
    let payload = std::fs::read(codex_home.join("models_cache.json")).ok()?;
    let value = serde_json::from_slice::<Value>(&payload).ok()?;
    value
        .get("client_version")
        .and_then(Value::as_str)
        .and_then(normalize_client_version)
}

fn sanitize_error(error: &str) -> String {
    error
        .replace("access_token", "token")
        .replace("Authorization", "auth")
        .chars()
        .take(240)
        .collect()
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_future_and_hidden_entries_without_a_fixed_model_list() {
        let payload = json!({
            "models": [
                { "slug": "gpt-6-astra", "visibility": "list", "supported_in_api": true },
                { "slug": "gpt-7-internal", "visibility": "hidden", "supported_in_api": true },
                { "slug": "gpt-6-fake", "visibility": "list", "supported_in_api": false }
            ]
        });
        let entries = parse_model_entries(&payload);
        assert_eq!(
            unique_slugs(&entries),
            vec!["gpt-6-astra", "gpt-7-internal", "gpt-6-fake"]
        );
        assert_eq!(visible_slugs(&entries), vec!["gpt-6-astra"]);
    }

    #[test]
    fn cache_serialization_does_not_include_authentication_fields() {
        let cache = CatalogCacheFile {
            version: CACHE_VERSION,
            accounts: BTreeMap::from([(
                "account-1".to_string(),
                CachedOfficialCatalog {
                    account_id: "account-1".to_string(),
                    fetched_at: 1,
                    client_version: "0.153.4".to_string(),
                    etag: Some("etag".to_string()),
                    source: "account_snapshot".to_string(),
                    models: vec![json!({"slug": "gpt-6-astra"})],
                },
            )]),
        };
        let text = serde_json::to_string(&cache).unwrap();
        assert!(text.contains("gpt-6-astra"));
        assert!(!text.contains("access_token"));
        assert!(!text.contains("Authorization"));
    }

    #[test]
    fn parses_bundled_cli_json_with_log_prefix() {
        let output = br#"loading bundled models...
{"models":[{"slug":"gpt-6-astra","visibility":"list","supported_in_api":true},{"slug":"gpt-7-internal","visibility":"hidden","supported_in_api":true}]}"#;
        let entries = parse_catalog_output(output);
        assert_eq!(
            unique_slugs(&entries),
            vec!["gpt-6-astra", "gpt-7-internal"]
        );
        assert_eq!(visible_slugs(&entries), vec!["gpt-6-astra"]);
    }

    #[test]
    fn parses_only_real_codex_cli_versions() {
        assert_eq!(
            parse_codex_cli_version_output("codex-cli 0.154.0\n"),
            Some("0.154.0".to_string())
        );
        assert_eq!(
            parse_codex_cli_version_output("codex-cli 0.154.0-alpha.2\n"),
            Some("0.154.0".to_string())
        );
        assert_eq!(parse_codex_cli_version_output("26.908.4834.0\n"), None);
        assert_eq!(
            parse_codex_cli_version_output("请在已有的应用会话中打开。\n"),
            None
        );
    }

    #[test]
    fn bundled_catalog_keeps_priority_and_appends_compatibility_models() {
        let bundled = vec![json!({
            "slug": "gpt-5.5",
            "display_name": "Bundled GPT-5.5",
            "visibility": "list",
            "supported_in_api": true
        })];
        let merged = merge_catalog_entries(bundled, compatibility_entries(true));
        assert_eq!(
            merged[0].get("display_name").and_then(Value::as_str),
            Some("Bundled GPT-5.5")
        );
        assert!(
            visible_slugs(&merged)
                .iter()
                .any(|slug| slug == "gpt-6-astra")
        );
    }

    #[test]
    fn latest_client_version_prefers_the_newest_numeric_source() {
        assert_eq!(
            latest_client_version(vec![
                "0.139.0".to_string(),
                "0.149.0".to_string(),
                "0.148.7".to_string(),
            ]),
            Some("0.149.0".to_string())
        );
    }

    #[test]
    fn etag_is_not_reused_for_forced_or_new_client_catalog_requests() {
        let snapshot = CachedOfficialCatalog {
            account_id: "account-1".to_string(),
            fetched_at: 1,
            client_version: "0.149.0".to_string(),
            etag: Some("etag-149".to_string()),
            source: "account_snapshot".to_string(),
            models: Vec::new(),
        };
        assert_eq!(
            cached_etag_for_request(Some(&snapshot), "0.149.0", false),
            Some("etag-149")
        );
        assert_eq!(
            cached_etag_for_request(Some(&snapshot), "0.154.0", false),
            None
        );
        assert_eq!(
            cached_etag_for_request(Some(&snapshot), "0.149.0", true),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn discovers_desktop_managed_codex_cli_versions() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp
            .path()
            .join("OpenAI")
            .join("Codex")
            .join("bin")
            .join("first")
            .join("codex.exe");
        let second = temp
            .path()
            .join("OpenAI")
            .join("Codex")
            .join("bin")
            .join("second")
            .join("codex.exe");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::create_dir_all(second.parent().unwrap()).unwrap();
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();

        let candidates = windows_local_codex_cli_candidates(temp.path());
        assert!(candidates.contains(&first));
        assert!(candidates.contains(&second));
    }

    #[test]
    fn active_account_catalog_is_isolated_from_other_accounts() {
        let _settings_guard = crate::paths::settings_path_test_guard();
        let temp = tempfile::tempdir().unwrap();
        let previous_catalog_path = crate::paths::set_official_model_catalog_path_for_tests(Some(
            temp.path().join("official-model-catalog.json"),
        ));
        let cache = CatalogCacheFile {
            version: CACHE_VERSION,
            accounts: BTreeMap::from([
                (
                    "account-a".to_string(),
                    CachedOfficialCatalog {
                        account_id: "account-a".to_string(),
                        fetched_at: now_ts(),
                        client_version: "0.1.0".to_string(),
                        etag: None,
                        source: "account_snapshot".to_string(),
                        models: vec![json!({
                            "slug": "gpt-6-astra",
                            "visibility": "list",
                            "supported_in_api": true
                        })],
                    },
                ),
                (
                    "account-b".to_string(),
                    CachedOfficialCatalog {
                        account_id: "account-b".to_string(),
                        fetched_at: now_ts(),
                        client_version: "0.1.0".to_string(),
                        etag: None,
                        source: "account_snapshot".to_string(),
                        models: vec![json!({
                            "slug": "gpt-6-fake",
                            "visibility": "list",
                            "supported_in_api": true
                        })],
                    },
                ),
            ]),
        };
        save_cache(&cache).unwrap();

        let account_a = BackendSettings {
            active_official_account_id: "account-a".to_string(),
            ..BackendSettings::default()
        };
        assert_eq!(
            visible_official_model_slugs_for_settings(&account_a),
            vec!["gpt-6-astra"]
        );
        assert!(
            trusted_official_model_slugs_for_settings(&account_a)
                .iter()
                .any(|slug| slug == "gpt-6-astra")
        );
        assert!(
            !trusted_official_model_slugs_for_settings(&account_a)
                .iter()
                .any(|slug| slug == "gpt-6-fake")
        );

        let account_b = BackendSettings {
            active_official_account_id: "account-b".to_string(),
            ..BackendSettings::default()
        };
        assert_eq!(
            visible_official_model_slugs_for_settings(&account_b),
            vec!["gpt-6-fake"]
        );
        let status = status_for_settings(&account_a, None);
        assert_eq!(status.source, "account_snapshot");
        assert_eq!(status.visible_models, vec!["gpt-6-astra"]);

        crate::paths::set_official_model_catalog_path_for_tests(previous_catalog_path);
    }
}
