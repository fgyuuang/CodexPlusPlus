use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, OsRng, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use anyhow::{Context, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const STORE_VERSION: u32 = 1;
const EXPORT_VERSION: u32 = 1;
const EXPORT_FORMAT: &str = "codex-plus-plus-official-accounts";
const SECRET_BACKEND_DPAPI: &str = "windows-dpapi-current-user";
#[cfg(not(windows))]
const SECRET_BACKEND_USER_FILE: &str = "current-user-file";
const DPAPI_ENTROPY: &[u8] = b"CodexPlusPlus.OfficialAccounts.v1";
const EXPORT_AAD: &[u8] = b"CodexPlusPlus.OfficialAccounts.Export.v1";
const EXPORT_MEMORY_KIB: u32 = 64 * 1024;
const EXPORT_ITERATIONS: u32 = 3;
const EXPORT_PARALLELISM: u32 = 1;
const MAX_IMPORT_BYTES: u64 = 16 * 1024 * 1024;

pub const DEFAULT_OAUTH_ISSUER: &str = "https://auth.openai.com";
pub const DEFAULT_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const DEFAULT_OAUTH_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
pub const DEFAULT_OAUTH_ORIGINATOR: &str = "codex_cli_rs";
pub const LOGIN_SESSION_TTL: Duration = Duration::from_secs(15 * 60);
pub const USAGE_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserLoginStart {
    pub login_id: String,
    pub auth_url: String,
    pub redirect_uri: String,
    pub expires_at: i64,
    #[serde(skip)]
    pub state: String,
    #[serde(skip)]
    pub code_verifier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLoginStart {
    pub login_id: String,
    pub verification_url: String,
    pub user_code: String,
    pub expires_at: i64,
    pub interval_seconds: u64,
    #[serde(skip)]
    pub device_auth_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeviceAuthorizationResponse {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OfficialUsageWindow {
    pub used_percent: Option<f64>,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OfficialUsageSnapshot {
    pub fetched_at: i64,
    pub primary: Option<OfficialUsageWindow>,
    pub secondary: Option<OfficialUsageWindow>,
    #[serde(default)]
    pub credits: Option<Value>,
    #[serde(default)]
    pub additional_rate_limits: Vec<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OfficialAccountSummary {
    pub id: String,
    pub name: String,
    pub email: String,
    pub group: String,
    pub tags: Vec<String>,
    pub sort: i64,
    pub enabled: bool,
    pub status: String,
    pub chatgpt_account_id: String,
    pub workspace_id: String,
    pub plan_type: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_refresh_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub usage: Option<OfficialUsageSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OfficialAccountPatch {
    pub name: Option<String>,
    pub group: Option<String>,
    pub tags: Option<Vec<String>>,
    pub sort: Option<i64>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OfficialAccountImportResult {
    pub imported: Vec<String>,
    pub updated: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfficialIdentity {
    subject: String,
    chatgpt_account_id: String,
    workspace_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfficialAccountRecord {
    #[serde(flatten)]
    summary: OfficialAccountSummary,
    identity: OfficialIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AccountsFile {
    version: u32,
    accounts: Vec<OfficialAccountRecord>,
}

impl Default for AccountsFile {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            accounts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecretPayload {
    version: u32,
    credentials: BTreeMap<String, Value>,
}

impl Default for SecretPayload {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            credentials: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecretEnvelope {
    version: u32,
    backend: String,
    payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportEnvelope {
    format: String,
    version: u32,
    kdf: ExportKdf,
    cipher: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportKdf {
    algorithm: String,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    salt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportPayload {
    version: u32,
    exported_at: i64,
    accounts: Vec<ExportAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExportAccount {
    record: OfficialAccountRecord,
    auth: Value,
}

#[derive(Debug, Clone)]
pub struct ParsedOfficialAuth {
    pub id: String,
    pub subject: String,
    pub email: String,
    pub name: String,
    pub chatgpt_account_id: String,
    pub workspace_id: String,
    pub plan_type: String,
    pub access_token_expires_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficialAuthMergeDecision {
    KeepStored,
    UseCandidate,
    Conflict,
}

#[derive(Debug, Clone)]
pub struct OfficialAccountStore {
    metadata_path: PathBuf,
    secrets_path: PathBuf,
    lock_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OfficialAccountMigrationResult {
    pub changed: bool,
    pub imported: usize,
    pub active_account_id: String,
}

impl Default for OfficialAccountStore {
    fn default() -> Self {
        Self::new(
            crate::paths::default_official_accounts_path(),
            crate::paths::default_official_account_secrets_path(),
        )
    }
}

impl OfficialAccountStore {
    pub fn new(metadata_path: PathBuf, secrets_path: PathBuf) -> Self {
        let lock_path = metadata_path.with_extension("lock");
        Self {
            metadata_path,
            secrets_path,
            lock_path,
        }
    }

    pub fn list(&self) -> anyhow::Result<Vec<OfficialAccountSummary>> {
        self.with_lock(|| {
            let mut accounts = self.load_accounts_unlocked()?.accounts;
            accounts.sort_by_key(|record| (record.summary.sort, record.summary.created_at));
            Ok(accounts.into_iter().map(|record| record.summary).collect())
        })
    }

    pub fn get(&self, account_id: &str) -> anyhow::Result<OfficialAccountSummary> {
        let account_id = account_id.trim();
        self.with_lock(|| {
            self.load_accounts_unlocked()?
                .accounts
                .into_iter()
                .find(|record| record.summary.id == account_id)
                .map(|record| record.summary)
                .context("官方账号不存在")
        })
    }

    pub async fn refresh_tokens(
        &self,
        account_id: &str,
        force: bool,
    ) -> anyhow::Result<OfficialAccountSummary> {
        let current = self.get_auth_json(account_id)?;
        let parsed = parse_official_auth(&current)?;
        if !force
            && parsed
                .access_token_expires_at
                .is_some_and(|expires| expires > now_ts() + 5 * 60)
        {
            return self.get(account_id);
        }
        match refresh_auth_json(&current).await {
            Ok(refreshed) => self.replace_auth_json(account_id, refreshed),
            Err(error) => {
                let _ = self.mark_status(account_id, "needsReauth");
                Err(error)
            }
        }
    }

    pub async fn refresh_usage(
        &self,
        account_id: &str,
        force_usage: bool,
    ) -> anyhow::Result<OfficialAccountSummary> {
        self.refresh_usage_inner(account_id, force_usage, true)
            .await
    }

    pub async fn refresh_usage_with_current_token(
        &self,
        account_id: &str,
        force_usage: bool,
    ) -> anyhow::Result<OfficialAccountSummary> {
        self.refresh_usage_inner(account_id, force_usage, false)
            .await
    }

    async fn refresh_usage_inner(
        &self,
        account_id: &str,
        force_usage: bool,
        refresh_expired_token: bool,
    ) -> anyhow::Result<OfficialAccountSummary> {
        let current = self.get(account_id)?;
        if !force_usage && usage_cache_is_fresh(current.usage.as_ref()) {
            return Ok(current);
        }
        if refresh_expired_token {
            let _ = self.refresh_tokens(account_id, false).await?;
        }
        let auth = self.get_auth_json(account_id)?;
        match fetch_usage(&auth).await {
            Ok(usage) => self.update_usage(account_id, usage),
            Err(error) => {
                let mut usage = current.usage.unwrap_or(OfficialUsageSnapshot {
                    fetched_at: now_ts(),
                    primary: None,
                    secondary: None,
                    credits: None,
                    additional_rate_limits: Vec::new(),
                    error: None,
                });
                usage.error = Some(error.to_string());
                self.update_usage(account_id, usage)?;
                Err(error)
            }
        }
    }

    pub fn get_auth_json(&self, account_id: &str) -> anyhow::Result<Value> {
        let account_id = account_id.trim();
        ensure!(!account_id.is_empty(), "官方账号 ID 不能为空");
        self.with_lock(|| {
            let accounts = self.load_accounts_unlocked()?;
            ensure!(
                accounts
                    .accounts
                    .iter()
                    .any(|record| record.summary.id == account_id),
                "官方账号不存在"
            );
            self.load_secrets_unlocked()?
                .credentials
                .remove(account_id)
                .context("官方账号凭据不存在")
        })
    }

    pub fn upsert_auth_json(&self, auth: Value) -> anyhow::Result<(OfficialAccountSummary, bool)> {
        let parsed = parse_official_auth(&auth)?;
        self.with_lock(|| {
            let mut accounts = self.load_accounts_unlocked()?;
            let mut secrets = self.load_secrets_unlocked()?;
            let now = now_ts();
            let existing = accounts
                .accounts
                .iter_mut()
                .find(|record| record.summary.id == parsed.id);
            let created = existing.is_none();
            let summary = if let Some(record) = existing {
                record.summary.email = parsed.email.clone();
                if record.summary.name.trim().is_empty() {
                    record.summary.name = preferred_account_name(&parsed);
                }
                record.summary.chatgpt_account_id = parsed.chatgpt_account_id.clone();
                record.summary.workspace_id = parsed.workspace_id.clone();
                record.summary.plan_type = parsed.plan_type.clone();
                record.summary.status = "ready".to_string();
                record.summary.updated_at = now;
                record.summary.last_refresh_at = Some(now);
                record.identity = parsed_identity(&parsed);
                record.summary.clone()
            } else {
                let sort = accounts
                    .accounts
                    .iter()
                    .map(|record| record.summary.sort)
                    .max()
                    .unwrap_or(-1)
                    + 1;
                let summary = OfficialAccountSummary {
                    id: parsed.id.clone(),
                    name: preferred_account_name(&parsed),
                    email: parsed.email.clone(),
                    group: String::new(),
                    tags: Vec::new(),
                    sort,
                    enabled: true,
                    status: "ready".to_string(),
                    chatgpt_account_id: parsed.chatgpt_account_id.clone(),
                    workspace_id: parsed.workspace_id.clone(),
                    plan_type: parsed.plan_type.clone(),
                    created_at: now,
                    updated_at: now,
                    last_refresh_at: Some(now),
                    last_used_at: None,
                    usage: None,
                };
                accounts.accounts.push(OfficialAccountRecord {
                    summary: summary.clone(),
                    identity: parsed_identity(&parsed),
                });
                summary
            };
            secrets
                .credentials
                .insert(parsed.id, normalize_auth_json(auth)?);
            self.save_state_unlocked(&accounts, &secrets)?;
            Ok((summary, created))
        })
    }

    pub fn update(
        &self,
        account_id: &str,
        patch: OfficialAccountPatch,
    ) -> anyhow::Result<OfficialAccountSummary> {
        let account_id = account_id.trim();
        self.with_lock(|| {
            let mut accounts = self.load_accounts_unlocked()?;
            let record = accounts
                .accounts
                .iter_mut()
                .find(|record| record.summary.id == account_id)
                .context("官方账号不存在")?;
            if let Some(name) = patch.name {
                let name = name.trim();
                ensure!(!name.is_empty(), "账号名称不能为空");
                record.summary.name = name.to_string();
            }
            if let Some(group) = patch.group {
                record.summary.group = group.trim().to_string();
            }
            if let Some(tags) = patch.tags {
                record.summary.tags = normalize_tags(tags);
            }
            if let Some(sort) = patch.sort {
                record.summary.sort = sort;
            }
            if let Some(enabled) = patch.enabled {
                record.summary.enabled = enabled;
            }
            record.summary.updated_at = now_ts();
            let summary = record.summary.clone();
            let secrets = self.load_secrets_unlocked()?;
            self.save_state_unlocked(&accounts, &secrets)?;
            Ok(summary)
        })
    }

    pub fn delete(&self, account_id: &str) -> anyhow::Result<bool> {
        let account_id = account_id.trim();
        self.with_lock(|| {
            let mut accounts = self.load_accounts_unlocked()?;
            let original_len = accounts.accounts.len();
            accounts
                .accounts
                .retain(|record| record.summary.id != account_id);
            if accounts.accounts.len() == original_len {
                return Ok(false);
            }
            let mut secrets = self.load_secrets_unlocked()?;
            secrets.credentials.remove(account_id);
            self.save_state_unlocked(&accounts, &secrets)?;
            Ok(true)
        })
    }

    pub fn replace_auth_json(
        &self,
        account_id: &str,
        auth: Value,
    ) -> anyhow::Result<OfficialAccountSummary> {
        let parsed = parse_official_auth(&auth)?;
        ensure!(parsed.id == account_id.trim(), "登录身份与目标账号不一致");
        self.upsert_auth_json(auth).map(|(summary, _)| summary)
    }

    pub fn update_usage(
        &self,
        account_id: &str,
        usage: OfficialUsageSnapshot,
    ) -> anyhow::Result<OfficialAccountSummary> {
        self.with_lock(|| {
            let mut accounts = self.load_accounts_unlocked()?;
            let record = accounts
                .accounts
                .iter_mut()
                .find(|record| record.summary.id == account_id.trim())
                .context("官方账号不存在")?;
            record.summary.usage = Some(usage);
            record.summary.updated_at = now_ts();
            let summary = record.summary.clone();
            let secrets = self.load_secrets_unlocked()?;
            self.save_state_unlocked(&accounts, &secrets)?;
            Ok(summary)
        })
    }

    pub fn mark_status(&self, account_id: &str, status: &str) -> anyhow::Result<()> {
        self.with_lock(|| {
            let mut accounts = self.load_accounts_unlocked()?;
            let record = accounts
                .accounts
                .iter_mut()
                .find(|record| record.summary.id == account_id.trim())
                .context("官方账号不存在")?;
            record.summary.status = status.trim().to_string();
            record.summary.updated_at = now_ts();
            let secrets = self.load_secrets_unlocked()?;
            self.save_state_unlocked(&accounts, &secrets)
        })
    }

    pub fn mark_used(&self, account_id: &str) -> anyhow::Result<()> {
        self.with_lock(|| {
            let mut accounts = self.load_accounts_unlocked()?;
            let record = accounts
                .accounts
                .iter_mut()
                .find(|record| record.summary.id == account_id.trim())
                .context("官方账号不存在")?;
            let now = now_ts();
            record.summary.last_used_at = Some(now);
            record.summary.updated_at = now;
            let secrets = self.load_secrets_unlocked()?;
            self.save_state_unlocked(&accounts, &secrets)
        })
    }

    pub fn export_encrypted(
        &self,
        account_ids: &[String],
        password: &str,
    ) -> anyhow::Result<Vec<u8>> {
        ensure!(password.chars().count() >= 8, "导出密码至少需要 8 个字符");
        let selected = account_ids
            .iter()
            .map(|id| id.trim())
            .filter(|id| !id.is_empty())
            .collect::<HashSet<_>>();
        ensure!(!selected.is_empty(), "请至少选择一个账号");
        self.with_lock(|| {
            let accounts = self.load_accounts_unlocked()?;
            let secrets = self.load_secrets_unlocked()?;
            let mut export_accounts = Vec::new();
            for record in accounts.accounts {
                if !selected.contains(record.summary.id.as_str()) {
                    continue;
                }
                let auth = secrets
                    .credentials
                    .get(&record.summary.id)
                    .cloned()
                    .context("所选账号缺少凭据")?;
                export_accounts.push(ExportAccount { record, auth });
            }
            ensure!(
                export_accounts.len() == selected.len(),
                "部分所选账号不存在"
            );
            encrypt_export_payload(
                &ExportPayload {
                    version: EXPORT_VERSION,
                    exported_at: now_ts(),
                    accounts: export_accounts,
                },
                password,
            )
        })
    }

    pub fn import_encrypted(
        &self,
        bytes: &[u8],
        password: &str,
    ) -> anyhow::Result<OfficialAccountImportResult> {
        ensure!(bytes.len() as u64 <= MAX_IMPORT_BYTES, "导入文件过大");
        let payload = decrypt_export_payload(bytes, password)?;
        let mut imported = Vec::new();
        let mut updated = Vec::new();
        let mut errors = Vec::new();
        for item in payload.accounts {
            match self.upsert_export_account(item) {
                Ok((id, true)) => imported.push(id),
                Ok((id, false)) => updated.push(id),
                Err(error) => errors.push(error.to_string()),
            }
        }
        Ok(OfficialAccountImportResult {
            imported,
            updated,
            errors,
        })
    }

    fn upsert_export_account(&self, item: ExportAccount) -> anyhow::Result<(String, bool)> {
        let parsed = parse_official_auth(&item.auth)?;
        ensure!(parsed.id == item.record.summary.id, "导出账号身份校验失败");
        let (mut summary, created) = self.upsert_auth_json(item.auth)?;
        if created {
            summary = self.update(
                &summary.id,
                OfficialAccountPatch {
                    name: Some(item.record.summary.name),
                    group: Some(item.record.summary.group),
                    tags: Some(item.record.summary.tags),
                    sort: Some(item.record.summary.sort),
                    enabled: Some(item.record.summary.enabled),
                },
            )?;
        }
        Ok((summary.id, created))
    }

    fn with_lock<T>(&self, operation: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
        if let Some(parent) = self.lock_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&self.lock_path)?;
        lock.lock_exclusive()?;
        let result = operation();
        let unlock_result = FileExt::unlock(&lock);
        match (result, unlock_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error.into()),
        }
    }

    fn load_accounts_unlocked(&self) -> anyhow::Result<AccountsFile> {
        match fs::read(&self.metadata_path) {
            Ok(bytes) => {
                let file: AccountsFile =
                    serde_json::from_slice(&bytes).context("官方账号元数据已损坏")?;
                ensure!(file.version == STORE_VERSION, "不支持的官方账号元数据版本");
                Ok(file)
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(AccountsFile::default()),
            Err(error) => Err(error.into()),
        }
    }

    fn load_secrets_unlocked(&self) -> anyhow::Result<SecretPayload> {
        let bytes = match fs::read(&self.secrets_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(SecretPayload::default());
            }
            Err(error) => return Err(error.into()),
        };
        let envelope: SecretEnvelope =
            serde_json::from_slice(&bytes).context("官方账号凭据文件已损坏")?;
        ensure!(
            envelope.version == STORE_VERSION,
            "不支持的官方账号凭据版本"
        );
        let protected = base64::engine::general_purpose::STANDARD
            .decode(envelope.payload)
            .context("官方账号凭据编码无效")?;
        let plaintext = unprotect_local_secret(&envelope.backend, &protected)?;
        let payload: SecretPayload =
            serde_json::from_slice(&plaintext).context("无法解密官方账号凭据")?;
        ensure!(
            payload.version == STORE_VERSION,
            "不支持的官方账号凭据内容版本"
        );
        Ok(payload)
    }

    fn save_state_unlocked(
        &self,
        accounts: &AccountsFile,
        secrets: &SecretPayload,
    ) -> anyhow::Result<()> {
        let secret_bytes = serde_json::to_vec(secrets)?;
        let (backend, protected) = protect_local_secret(&secret_bytes)?;
        let envelope = SecretEnvelope {
            version: STORE_VERSION,
            backend,
            payload: base64::engine::general_purpose::STANDARD.encode(protected),
        };
        crate::settings::atomic_write(&self.secrets_path, &serde_json::to_vec_pretty(&envelope)?)?;
        restrict_file_to_current_user(&self.secrets_path)?;
        crate::settings::atomic_write(&self.metadata_path, &serde_json::to_vec_pretty(accounts)?)?;
        restrict_file_to_current_user(&self.metadata_path)?;
        Ok(())
    }
}

pub fn migrate_legacy_official_accounts(
    settings: &mut crate::settings::BackendSettings,
    codex_home: &Path,
) -> anyhow::Result<OfficialAccountMigrationResult> {
    let store = OfficialAccountStore::default();
    migrate_legacy_official_accounts_with_store(settings, codex_home, &store)
}

fn migrate_legacy_official_accounts_with_store(
    settings: &mut crate::settings::BackendSettings,
    codex_home: &Path,
    store: &OfficialAccountStore,
) -> anyhow::Result<OfficialAccountMigrationResult> {
    let selected_relay_id = settings.official_login_relay_id.trim().to_string();
    let mut imported_ids = BTreeMap::new();
    let mut imported = 0usize;

    for profile in &settings.relay_profiles {
        if profile.relay_mode != crate::settings::RelayMode::Official
            || profile.official_mix_api_key
            || profile.auth_contents.trim().is_empty()
        {
            continue;
        }
        let Ok(auth) = serde_json::from_str::<Value>(&profile.auth_contents) else {
            continue;
        };
        if parse_official_auth(&auth).is_err() {
            continue;
        }
        let (summary, created) = store.upsert_auth_json(auth)?;
        imported += usize::from(created);
        imported_ids.insert(profile.id.clone(), summary.id);
    }

    let live_auth = fs::read(codex_home.join("auth.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(|auth| parse_official_auth(auth).is_ok());
    let live_account_id = if let Some(auth) = live_auth {
        let (summary, created) = store.upsert_auth_json(auth)?;
        imported += usize::from(created);
        Some(summary.id)
    } else {
        None
    };

    let previous_active = settings.active_official_account_id.trim().to_string();
    let active_account_id = if !previous_active.is_empty()
        && store
            .list()?
            .iter()
            .any(|account| account.id == previous_active && account.enabled)
    {
        previous_active
    } else {
        imported_ids
            .get(&selected_relay_id)
            .cloned()
            .or(live_account_id)
            .or_else(|| {
                store
                    .list()
                    .ok()?
                    .into_iter()
                    .find(|account| account.enabled)
                    .map(|account| account.id)
            })
            .unwrap_or_default()
    };

    let mut changed = settings.active_official_account_id != active_account_id;
    settings.active_official_account_id = active_account_id.clone();
    for profile in &mut settings.relay_profiles {
        if profile.relay_mode == crate::settings::RelayMode::Official
            && !profile.official_mix_api_key
            && serde_json::from_str::<Value>(&profile.auth_contents)
                .ok()
                .is_some_and(|auth| parse_official_auth(&auth).is_ok())
        {
            profile.auth_contents.clear();
            changed = true;
        }
    }
    Ok(OfficialAccountMigrationResult {
        changed,
        imported,
        active_account_id,
    })
}

pub fn parse_official_auth(auth: &Value) -> anyhow::Result<ParsedOfficialAuth> {
    let object = auth.as_object().context("auth.json 必须是 JSON 对象")?;
    let auth_mode = object
        .get("auth_mode")
        .and_then(Value::as_str)
        .unwrap_or("chatgpt");
    ensure!(
        auth_mode.eq_ignore_ascii_case("chatgpt"),
        "不是 ChatGPT 官方登录凭据"
    );
    let access_token = auth_token(auth, "access_token").context("缺少 access_token")?;
    let id_token = auth_token(auth, "id_token").unwrap_or(access_token);
    let id_claims = jwt_claims(id_token).context("id_token 不是有效 JWT")?;
    let access_claims = jwt_claims(access_token).unwrap_or_else(|| id_claims.clone());
    let subject = claim_string(&id_claims, &["sub"])
        .or_else(|| claim_string(&access_claims, &["sub"]))
        .context("官方令牌缺少 sub")?;
    let chatgpt_account_id = nested_claim_string(
        &access_claims,
        "https://api.openai.com/auth",
        &["chatgpt_account_id"],
    )
    .or_else(|| claim_string(&access_claims, &["chatgpt_account_id"]))
    .or_else(|| auth_token(auth, "account_id").map(ToString::to_string))
    .unwrap_or_default();
    let workspace_id = claim_string(
        &access_claims,
        &["workspace_id", "organization_id", "org_id"],
    )
    .or_else(|| {
        nested_claim_string(
            &access_claims,
            "https://api.openai.com/auth",
            &["workspace_id", "organization_id", "org_id"],
        )
    })
    .unwrap_or_else(|| chatgpt_account_id.clone());
    ensure!(
        !chatgpt_account_id.is_empty() || !workspace_id.is_empty(),
        "官方令牌缺少 ChatGPT account/workspace 标识"
    );
    let email = claim_string(&id_claims, &["email"])
        .or_else(|| nested_claim_string(&id_claims, "https://api.openai.com/profile", &["email"]))
        .unwrap_or_default();
    let name = nested_claim_string(&id_claims, "https://api.openai.com/profile", &["name"])
        .unwrap_or_default();
    let plan_type = nested_claim_string(
        &access_claims,
        "https://api.openai.com/auth",
        &["chatgpt_plan_type"],
    )
    .or_else(|| claim_string(&access_claims, &["chatgpt_plan_type"]))
    .unwrap_or_default();
    let id = stable_account_id(&subject, &chatgpt_account_id, &workspace_id);
    Ok(ParsedOfficialAuth {
        id,
        subject,
        email,
        name,
        chatgpt_account_id,
        workspace_id,
        plan_type,
        access_token_expires_at: access_claims.get("exp").and_then(Value::as_i64),
    })
}

pub fn begin_browser_login() -> anyhow::Result<BrowserLoginStart> {
    let state = random_urlsafe(32);
    let code_verifier = random_urlsafe(64);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(code_verifier.as_bytes()));
    let mut url = url::Url::parse(&format!("{DEFAULT_OAUTH_ISSUER}/oauth/authorize"))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", DEFAULT_OAUTH_CLIENT_ID)
        .append_pair("redirect_uri", DEFAULT_OAUTH_REDIRECT_URI)
        .append_pair(
            "scope",
            "openid profile email offline_access api.connectors.read api.connectors.invoke",
        )
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("state", &state)
        .append_pair("originator", DEFAULT_OAUTH_ORIGINATOR)
        .append_pair("prompt", "login");
    Ok(BrowserLoginStart {
        login_id: state.clone(),
        auth_url: url.to_string(),
        redirect_uri: DEFAULT_OAUTH_REDIRECT_URI.to_string(),
        expires_at: now_ts() + LOGIN_SESSION_TTL.as_secs() as i64,
        state,
        code_verifier,
    })
}

pub fn parse_callback_url(callback_url: &str, expected_state: &str) -> anyhow::Result<String> {
    let url = url::Url::parse(callback_url).context("OAuth 回调地址无效")?;
    let params = url.query_pairs().collect::<BTreeMap<_, _>>();
    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map(|value| value.as_ref())
            .unwrap_or(error.as_ref());
        anyhow::bail!("OAuth 登录失败：{description}");
    }
    let state = params.get("state").context("OAuth 回调缺少 state")?;
    ensure!(state.as_ref() == expected_state, "OAuth state 校验失败");
    params
        .get("code")
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
        .context("OAuth 回调缺少 code")
}

pub async fn complete_browser_login(flow: &BrowserLoginStart, code: &str) -> anyhow::Result<Value> {
    ensure!(now_ts() <= flow.expires_at, "OAuth 登录会话已过期");
    let response = auth_http_client()?
        .post(format!("{DEFAULT_OAUTH_ISSUER}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", flow.redirect_uri.as_str()),
            ("client_id", DEFAULT_OAUTH_CLIENT_ID),
            ("code_verifier", flow.code_verifier.as_str()),
        ])
        .send()
        .await
        .context("连接 OAuth token 接口失败")?;
    let tokens: OAuthTokenResponse = read_auth_json_response(response, "OAuth token 交换").await?;
    auth_json_from_oauth_response(tokens, None)
}

pub async fn begin_device_login() -> anyhow::Result<DeviceLoginStart> {
    let response = auth_http_client()?
        .post(format!(
            "{DEFAULT_OAUTH_ISSUER}/api/accounts/deviceauth/usercode"
        ))
        .json(&json!({ "client_id": DEFAULT_OAUTH_CLIENT_ID }))
        .send()
        .await
        .context("连接设备码接口失败")?;
    let value: Value = read_auth_json_response(response, "请求设备码").await?;
    let user_code = value
        .get("user_code")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("设备码响应缺少 user_code")?
        .to_string();
    let device_auth_id = value
        .get("device_auth_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("设备码响应缺少 device_auth_id")?
        .to_string();
    let interval_seconds = value
        .get("interval")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .unwrap_or(5)
        .clamp(1, 30);
    Ok(DeviceLoginStart {
        login_id: random_urlsafe(32),
        verification_url: format!("{DEFAULT_OAUTH_ISSUER}/codex/device"),
        user_code,
        expires_at: now_ts() + LOGIN_SESSION_TTL.as_secs() as i64,
        interval_seconds,
        device_auth_id,
    })
}

pub async fn complete_device_login(flow: &DeviceLoginStart) -> anyhow::Result<Value> {
    let mut interval = Duration::from_secs(flow.interval_seconds.clamp(1, 30));
    loop {
        ensure!(now_ts() <= flow.expires_at, "设备码登录已过期");
        let response = auth_http_client()?
            .post(format!(
                "{DEFAULT_OAUTH_ISSUER}/api/accounts/deviceauth/token"
            ))
            .json(&json!({
                "device_auth_id": flow.device_auth_id,
                "user_code": flow.user_code
            }))
            .send()
            .await
            .context("轮询设备码状态失败")?;
        let status = response.status();
        if status.is_success() {
            let authorization: DeviceAuthorizationResponse =
                response.json().await.context("设备码授权响应无效")?;
            let device_redirect = format!("{DEFAULT_OAUTH_ISSUER}/deviceauth/callback");
            let token_response = auth_http_client()?
                .post(format!("{DEFAULT_OAUTH_ISSUER}/oauth/token"))
                .form(&[
                    ("grant_type", "authorization_code"),
                    ("code", authorization.authorization_code.as_str()),
                    ("redirect_uri", device_redirect.as_str()),
                    ("client_id", DEFAULT_OAUTH_CLIENT_ID),
                    ("code_verifier", authorization.code_verifier.as_str()),
                ])
                .send()
                .await
                .context("交换设备码令牌失败")?;
            let tokens: OAuthTokenResponse =
                read_auth_json_response(token_response, "设备码 token 交换").await?;
            return auth_json_from_oauth_response(tokens, None);
        }
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND {
            tokio::time::sleep(interval).await;
            continue;
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            interval = (interval + Duration::from_secs(5)).min(Duration::from_secs(30));
            tokio::time::sleep(interval).await;
            continue;
        }
        let message = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "设备码登录失败（HTTP {}）：{}",
            status.as_u16(),
            summarize_remote_error(&message)
        );
    }
}

pub async fn refresh_auth_json(auth: &Value) -> anyhow::Result<Value> {
    let refresh_token = auth_token(auth, "refresh_token").context("账号缺少 refresh_token")?;
    let response = auth_http_client()?
        .post(format!("{DEFAULT_OAUTH_ISSUER}/oauth/token"))
        .form(&[
            ("client_id", DEFAULT_OAUTH_CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", "openid profile email"),
        ])
        .send()
        .await
        .context("连接 refresh token 接口失败")?;
    let tokens: OAuthTokenResponse = read_auth_json_response(response, "刷新官方令牌").await?;
    auth_json_from_oauth_response(tokens, Some(auth))
}

pub async fn fetch_usage(auth: &Value) -> anyhow::Result<OfficialUsageSnapshot> {
    let parsed = parse_official_auth(auth)?;
    let access_token = auth_token(auth, "access_token").context("账号缺少 access_token")?;
    let account_id = if parsed.chatgpt_account_id.is_empty() {
        parsed.workspace_id.as_str()
    } else {
        parsed.chatgpt_account_id.as_str()
    };
    let mut request = auth_http_client()?
        .get("https://chatgpt.com/backend-api/wham/usage")
        .bearer_auth(access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header("OpenAI-Beta", "codex-1");
    if !account_id.is_empty() {
        request = request.header("ChatGPT-Account-ID", account_id);
    }
    let response = request.send().await.context("连接官方用量接口失败")?;
    let value: Value = read_auth_json_response(response, "读取官方用量").await?;
    Ok(parse_usage_snapshot(&value))
}

fn auth_json_from_oauth_response(
    response: OAuthTokenResponse,
    previous: Option<&Value>,
) -> anyhow::Result<Value> {
    let previous_id = previous.and_then(|auth| auth_token(auth, "id_token"));
    let previous_refresh = previous.and_then(|auth| auth_token(auth, "refresh_token"));
    let id_token = response
        .id_token
        .as_deref()
        .or(previous_id)
        .context("OAuth 响应缺少 id_token")?
        .to_string();
    let refresh_token = response
        .refresh_token
        .as_deref()
        .or(previous_refresh)
        .context("OAuth 响应缺少 refresh_token")?
        .to_string();
    auth_json_from_tokens(id_token, response.access_token, refresh_token)
}

async fn read_auth_json_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
    action: &str,
) -> anyhow::Result<T> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "{action}失败（HTTP {}）：{}",
            status.as_u16(),
            summarize_remote_error(&body)
        );
    }
    response
        .json()
        .await
        .with_context(|| format!("{action}响应不是有效 JSON"))
}

fn auth_http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .user_agent(format!("codex-plus-plus/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("创建官方认证 HTTP 客户端失败")
}

fn random_urlsafe(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn summarize_remote_error(body: &str) -> String {
    let candidate = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("error_description"))
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| body.trim().to_string());
    let sanitized = candidate.split_whitespace().collect::<Vec<_>>().join(" ");
    if sanitized.is_empty() {
        "远端未返回错误详情".to_string()
    } else {
        sanitized.chars().take(300).collect()
    }
}

pub fn auth_token<'a>(auth: &'a Value, key: &str) -> Option<&'a str> {
    auth.get("tokens")
        .and_then(|tokens| tokens.get(key))
        .and_then(Value::as_str)
        .or_else(|| auth.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub fn official_auth_merge_decision(
    stored: &Value,
    candidate: &Value,
) -> anyhow::Result<OfficialAuthMergeDecision> {
    let stored_identity = parse_official_auth(stored)?;
    let candidate_identity = parse_official_auth(candidate)?;
    ensure!(
        stored_identity.id == candidate_identity.id,
        "登录身份与已保存账号不一致"
    );

    if stored == candidate {
        return Ok(OfficialAuthMergeDecision::KeepStored);
    }

    match compare_official_auth_revision(stored, candidate)? {
        Some(Ordering::Greater) => return Ok(OfficialAuthMergeDecision::UseCandidate),
        Some(Ordering::Less) => return Ok(OfficialAuthMergeDecision::KeepStored),
        Some(Ordering::Equal) | None => {}
    }

    if auth_token(stored, "refresh_token") == auth_token(candidate, "refresh_token") {
        Ok(OfficialAuthMergeDecision::UseCandidate)
    } else {
        Ok(OfficialAuthMergeDecision::Conflict)
    }
}

fn compare_official_auth_revision(
    stored: &Value,
    candidate: &Value,
) -> anyhow::Result<Option<Ordering>> {
    let stored_parsed = parse_official_auth(stored)?;
    let candidate_parsed = parse_official_auth(candidate)?;
    if let Some(ordering) = compare_optional_revision(
        stored_parsed.access_token_expires_at,
        candidate_parsed.access_token_expires_at,
    ) {
        return Ok(Some(ordering));
    }

    Ok(compare_optional_revision(
        auth_last_refresh_at(stored),
        auth_last_refresh_at(candidate),
    ))
}

fn compare_optional_revision(stored: Option<i64>, candidate: Option<i64>) -> Option<Ordering> {
    match (stored, candidate) {
        (Some(stored), Some(candidate)) if stored != candidate => Some(candidate.cmp(&stored)),
        (None, Some(_)) => Some(Ordering::Greater),
        (Some(_), None) => Some(Ordering::Less),
        _ => None,
    }
}

fn auth_last_refresh_at(auth: &Value) -> Option<i64> {
    auth.get("last_refresh")
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value.trim()).ok())
        .map(|value| value.timestamp())
}

pub fn normalize_auth_json(mut auth: Value) -> anyhow::Result<Value> {
    let parsed = parse_official_auth(&auth)?;
    let object = auth.as_object_mut().context("auth.json 必须是 JSON 对象")?;
    object.insert(
        "auth_mode".to_string(),
        Value::String("chatgpt".to_string()),
    );
    object.remove("OPENAI_API_KEY");
    if let Some(tokens) = object.get_mut("tokens").and_then(Value::as_object_mut) {
        if !parsed.chatgpt_account_id.is_empty() {
            tokens.insert(
                "account_id".to_string(),
                Value::String(parsed.chatgpt_account_id),
            );
        }
    }
    Ok(auth)
}

pub fn auth_json_from_tokens(
    id_token: String,
    access_token: String,
    refresh_token: String,
) -> anyhow::Result<Value> {
    let claims = jwt_claims(&access_token)
        .or_else(|| jwt_claims(&id_token))
        .context("OAuth 返回的令牌不是有效 JWT")?;
    let account_id = nested_claim_string(
        &claims,
        "https://api.openai.com/auth",
        &["chatgpt_account_id"],
    )
    .or_else(|| claim_string(&claims, &["chatgpt_account_id"]))
    .unwrap_or_default();
    normalize_auth_json(json!({
        "auth_mode": "chatgpt",
        "tokens": {
            "id_token": id_token,
            "access_token": access_token,
            "refresh_token": refresh_token,
            "account_id": account_id
        },
        "last_refresh": chrono::Utc::now().to_rfc3339()
    }))
}

pub fn parse_usage_snapshot(value: &Value) -> OfficialUsageSnapshot {
    let window = |name: &str| {
        let base = format!("/rate_limit/{name}");
        let used_percent = value
            .pointer(&format!("{base}/used_percent"))
            .and_then(Value::as_f64);
        let window_minutes = value
            .pointer(&format!("{base}/limit_window_seconds"))
            .and_then(Value::as_i64)
            .map(|seconds| (seconds + 59) / 60);
        let resets_at = value
            .pointer(&format!("{base}/reset_at"))
            .and_then(Value::as_i64);
        (used_percent.is_some() || window_minutes.is_some() || resets_at.is_some()).then_some(
            OfficialUsageWindow {
                used_percent,
                window_minutes,
                resets_at,
            },
        )
    };
    let additional_rate_limits = value
        .get("additional_rate_limits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    OfficialUsageSnapshot {
        fetched_at: now_ts(),
        primary: window("primary_window"),
        secondary: window("secondary_window"),
        credits: value.get("credits").cloned(),
        additional_rate_limits,
        error: None,
    }
}

pub fn usage_cache_is_fresh(usage: Option<&OfficialUsageSnapshot>) -> bool {
    usage.is_some_and(|usage| {
        now_ts().saturating_sub(usage.fetched_at) < USAGE_CACHE_TTL.as_secs() as i64
    })
}

fn parsed_identity(parsed: &ParsedOfficialAuth) -> OfficialIdentity {
    OfficialIdentity {
        subject: parsed.subject.clone(),
        chatgpt_account_id: parsed.chatgpt_account_id.clone(),
        workspace_id: parsed.workspace_id.clone(),
    }
}

fn preferred_account_name(parsed: &ParsedOfficialAuth) -> String {
    if !parsed.name.trim().is_empty() {
        parsed.name.trim().to_string()
    } else if !parsed.email.trim().is_empty() {
        parsed.email.trim().to_string()
    } else if !parsed.chatgpt_account_id.trim().is_empty() {
        parsed.chatgpt_account_id.trim().to_string()
    } else {
        parsed.id.chars().take(12).collect()
    }
}

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    tags.into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .filter(|tag| seen.insert(tag.to_ascii_lowercase()))
        .collect()
}

fn stable_account_id(subject: &str, account_id: &str, workspace_id: &str) -> String {
    let digest = Sha256::digest(format!("{subject}\n{account_id}\n{workspace_id}").as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn claim_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

fn nested_claim_string(value: &Value, namespace: &str, keys: &[&str]) -> Option<String> {
    value
        .get(namespace)
        .and_then(|value| claim_string(value, keys))
}

fn encrypt_export_payload(payload: &ExportPayload, password: &str) -> anyhow::Result<Vec<u8>> {
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    let key = derive_export_key(password, &salt)?;
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow::anyhow!("无法初始化导出加密"))?;
    let plaintext = serde_json::to_vec(payload)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: EXPORT_AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("导出加密失败"))?;
    let envelope = ExportEnvelope {
        format: EXPORT_FORMAT.to_string(),
        version: EXPORT_VERSION,
        kdf: ExportKdf {
            algorithm: "argon2id".to_string(),
            memory_kib: EXPORT_MEMORY_KIB,
            iterations: EXPORT_ITERATIONS,
            parallelism: EXPORT_PARALLELISM,
            salt: base64::engine::general_purpose::STANDARD.encode(salt),
        },
        cipher: "aes-256-gcm".to_string(),
        nonce: base64::engine::general_purpose::STANDARD.encode(nonce),
        ciphertext: base64::engine::general_purpose::STANDARD.encode(ciphertext),
    };
    Ok(serde_json::to_vec_pretty(&envelope)?)
}

fn decrypt_export_payload(bytes: &[u8], password: &str) -> anyhow::Result<ExportPayload> {
    ensure!(password.chars().count() >= 8, "导入密码至少需要 8 个字符");
    let envelope: ExportEnvelope =
        serde_json::from_slice(bytes).context("不是有效的官方账号备份包")?;
    ensure!(envelope.format == EXPORT_FORMAT, "不支持的备份包格式");
    ensure!(envelope.version == EXPORT_VERSION, "不支持的备份包版本");
    ensure!(envelope.kdf.algorithm == "argon2id", "不支持的密钥派生算法");
    ensure!(envelope.cipher == "aes-256-gcm", "不支持的加密算法");
    ensure!(
        envelope.kdf.memory_kib == EXPORT_MEMORY_KIB
            && envelope.kdf.iterations == EXPORT_ITERATIONS
            && envelope.kdf.parallelism == EXPORT_PARALLELISM,
        "备份包使用了不受支持的 Argon2 参数"
    );
    let salt = base64::engine::general_purpose::STANDARD.decode(envelope.kdf.salt)?;
    let nonce = base64::engine::general_purpose::STANDARD.decode(envelope.nonce)?;
    ensure!(
        salt.len() == 16 && nonce.len() == 12,
        "备份包盐值或 nonce 无效"
    );
    let ciphertext = base64::engine::general_purpose::STANDARD.decode(envelope.ciphertext)?;
    let key = derive_export_key(password, &salt)?;
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow::anyhow!("无法初始化导入解密"))?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: EXPORT_AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("密码错误或备份包已被篡改"))?;
    let payload: ExportPayload = serde_json::from_slice(&plaintext).context("备份包内容无效")?;
    ensure!(payload.version == EXPORT_VERSION, "不支持的备份内容版本");
    Ok(payload)
}

fn derive_export_key(password: &str, salt: &[u8]) -> anyhow::Result<[u8; 32]> {
    let params = Params::new(
        EXPORT_MEMORY_KIB,
        EXPORT_ITERATIONS,
        EXPORT_PARALLELISM,
        Some(32),
    )
    .map_err(|error| anyhow::anyhow!("Argon2 参数无效：{error}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|error| anyhow::anyhow!("派生导出密钥失败：{error}"))?;
    Ok(key)
}

#[cfg(windows)]
fn protect_local_secret(bytes: &[u8]) -> anyhow::Result<(String, Vec<u8>)> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    };
    use windows::core::PCWSTR;

    let input_len = u32::try_from(bytes.len()).context("凭据文件过大")?;
    let entropy_len = u32::try_from(DPAPI_ENTROPY.len()).unwrap_or_default();
    let input = CRYPT_INTEGER_BLOB {
        cbData: input_len,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: entropy_len,
        pbData: DPAPI_ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        let protected = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData.cast()));
        Ok((SECRET_BACKEND_DPAPI.to_string(), protected))
    }
}

#[cfg(windows)]
fn unprotect_local_secret(backend: &str, bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };

    ensure!(
        backend == SECRET_BACKEND_DPAPI,
        "当前平台不支持此凭据保护后端"
    );
    let input_len = u32::try_from(bytes.len()).context("凭据文件过大")?;
    let entropy_len = u32::try_from(DPAPI_ENTROPY.len()).unwrap_or_default();
    let input = CRYPT_INTEGER_BLOB {
        cbData: input_len,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: entropy_len,
        pbData: DPAPI_ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        let plaintext = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData.cast()));
        Ok(plaintext)
    }
}

#[cfg(not(windows))]
fn protect_local_secret(bytes: &[u8]) -> anyhow::Result<(String, Vec<u8>)> {
    Ok((SECRET_BACKEND_USER_FILE.to_string(), bytes.to_vec()))
}

#[cfg(not(windows))]
fn unprotect_local_secret(backend: &str, bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    ensure!(
        backend == SECRET_BACKEND_USER_FILE,
        "当前平台不支持此凭据保护后端"
    );
    Ok(bytes.to_vec())
}

fn restrict_file_to_current_user(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn read_import_file(path: &Path) -> anyhow::Result<Vec<u8>> {
    let metadata = fs::metadata(path)?;
    ensure!(metadata.is_file(), "导入路径不是文件");
    ensure!(metadata.len() <= MAX_IMPORT_BYTES, "导入文件超过 16 MiB");
    fs::read(path).map_err(Into::into)
}

pub fn write_export_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    ensure!(!bytes.is_empty(), "导出内容为空");
    crate::settings::atomic_write(path, bytes)?;
    restrict_file_to_current_user(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(payload: Value) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\"}");
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        format!("{header}.{payload}.sig")
    }

    fn sample_auth(subject: &str, account_id: &str) -> Value {
        sample_auth_with_revision(subject, account_id, now_ts() + 3600, "refresh-test", None)
    }

    fn sample_auth_with_revision(
        subject: &str,
        account_id: &str,
        expires_at: i64,
        refresh_token: &str,
        last_refresh: Option<&str>,
    ) -> Value {
        let id_token = jwt(json!({
            "sub": subject,
            "email": format!("{subject}@example.com"),
            "https://api.openai.com/profile": { "name": subject }
        }));
        let access_token = jwt(json!({
            "sub": subject,
            "exp": expires_at,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_plan_type": "plus"
            }
        }));
        let mut auth = json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "id_token": id_token,
                "access_token": access_token,
                "refresh_token": refresh_token,
                "account_id": account_id
            }
        });
        if let Some(last_refresh) = last_refresh {
            auth["last_refresh"] = Value::String(last_refresh.to_string());
        }
        auth
    }

    #[test]
    fn parses_and_deduplicates_account_identity() {
        let first = parse_official_auth(&sample_auth("user-1", "acct-1")).unwrap();
        let second = parse_official_auth(&sample_auth("user-1", "acct-1")).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.email, "user-1@example.com");
        assert_eq!(first.plan_type, "plus");
    }

    #[test]
    fn auth_merge_keeps_newer_stored_refresh_token() {
        let stored = sample_auth_with_revision(
            "user-1",
            "acct-1",
            2_000,
            "refresh-new",
            Some("2026-01-01T00:10:00Z"),
        );
        let stale_live = sample_auth_with_revision(
            "user-1",
            "acct-1",
            1_000,
            "refresh-old",
            Some("2026-01-01T00:00:00Z"),
        );

        assert_eq!(
            official_auth_merge_decision(&stored, &stale_live).unwrap(),
            OfficialAuthMergeDecision::KeepStored
        );
    }

    #[test]
    fn auth_merge_accepts_newer_live_refresh_token() {
        let stored = sample_auth_with_revision(
            "user-1",
            "acct-1",
            1_000,
            "refresh-old",
            Some("2026-01-01T00:00:00Z"),
        );
        let newer_live = sample_auth_with_revision(
            "user-1",
            "acct-1",
            2_000,
            "refresh-new",
            Some("2026-01-01T00:10:00Z"),
        );

        assert_eq!(
            official_auth_merge_decision(&stored, &newer_live).unwrap(),
            OfficialAuthMergeDecision::UseCandidate
        );
    }

    #[test]
    fn auth_merge_rejects_ambiguous_rotated_refresh_tokens() {
        let stored = sample_auth_with_revision(
            "user-1",
            "acct-1",
            2_000,
            "refresh-a",
            Some("2026-01-01T00:10:00Z"),
        );
        let live = sample_auth_with_revision(
            "user-1",
            "acct-1",
            2_000,
            "refresh-b",
            Some("2026-01-01T00:10:00Z"),
        );

        assert_eq!(
            official_auth_merge_decision(&stored, &live).unwrap(),
            OfficialAuthMergeDecision::Conflict
        );
    }

    #[test]
    fn auth_merge_rejects_a_different_account() {
        let stored = sample_auth("user-1", "acct-1");
        let other = sample_auth("user-2", "acct-2");

        assert!(official_auth_merge_decision(&stored, &other).is_err());
    }

    #[test]
    fn account_store_round_trips_protected_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let store = OfficialAccountStore::new(
            temp.path().join("accounts.json"),
            temp.path().join("secrets.json"),
        );
        let auth = sample_auth("user-1", "acct-1");
        let (account, created) = store.upsert_auth_json(auth.clone()).unwrap();
        assert!(created);
        assert_eq!(store.list().unwrap().len(), 1);
        assert_eq!(store.get_auth_json(&account.id).unwrap(), auth);
        assert!(
            !fs::read_to_string(temp.path().join("secrets.json"))
                .unwrap()
                .contains("refresh-test")
        );
    }

    #[test]
    fn encrypted_export_rejects_wrong_password_and_merges_identity() {
        let source = tempfile::tempdir().unwrap();
        let source_store = OfficialAccountStore::new(
            source.path().join("accounts.json"),
            source.path().join("secrets.json"),
        );
        let (account, _) = source_store
            .upsert_auth_json(sample_auth("user-1", "acct-1"))
            .unwrap();
        let bytes = source_store
            .export_encrypted(std::slice::from_ref(&account.id), "password-123")
            .unwrap();
        assert!(
            source_store
                .import_encrypted(&bytes, "incorrect-password")
                .is_err()
        );

        let target = tempfile::tempdir().unwrap();
        let target_store = OfficialAccountStore::new(
            target.path().join("accounts.json"),
            target.path().join("secrets.json"),
        );
        let first = target_store
            .import_encrypted(&bytes, "password-123")
            .unwrap();
        assert_eq!(first.imported, vec![account.id.clone()]);
        let second = target_store
            .import_encrypted(&bytes, "password-123")
            .unwrap();
        assert_eq!(second.updated, vec![account.id]);
        assert_eq!(target_store.list().unwrap().len(), 1);
    }

    #[test]
    fn usage_parser_handles_primary_and_secondary_windows() {
        let usage = parse_usage_snapshot(&json!({
            "rate_limit": {
                "primary_window": { "used_percent": 25.0, "limit_window_seconds": 18000, "reset_at": 100 },
                "secondary_window": { "used_percent": 50.0, "limit_window_seconds": 604800, "reset_at": 200 }
            }
        }));
        assert_eq!(usage.primary.unwrap().window_minutes, Some(300));
        assert_eq!(usage.secondary.unwrap().window_minutes, Some(10080));
    }

    #[test]
    fn legacy_official_profile_migrates_once_and_clears_frontend_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let store = OfficialAccountStore::new(
            temp.path().join("accounts.json"),
            temp.path().join("secrets.json"),
        );
        let mut profile = crate::settings::RelayProfile::default();
        profile.id = "legacy-official".to_string();
        profile.relay_mode = crate::settings::RelayMode::Official;
        profile.auth_contents = serde_json::to_string(&sample_auth("user-1", "acct-1")).unwrap();
        let mut settings = crate::settings::BackendSettings {
            official_login_relay_id: profile.id.clone(),
            relay_profiles: vec![profile],
            ..crate::settings::BackendSettings::default()
        };

        let first = migrate_legacy_official_accounts_with_store(
            &mut settings,
            &temp.path().join("codex"),
            &store,
        )
        .unwrap();

        assert!(first.changed);
        assert_eq!(first.imported, 1);
        assert!(!first.active_account_id.is_empty());
        assert!(settings.relay_profiles[0].auth_contents.is_empty());
        assert!(store.get_auth_json(&first.active_account_id).is_ok());

        let second = migrate_legacy_official_accounts_with_store(
            &mut settings,
            &temp.path().join("codex"),
            &store,
        )
        .unwrap();
        assert!(!second.changed);
        assert_eq!(second.imported, 0);
    }
}
