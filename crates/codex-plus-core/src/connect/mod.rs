pub mod app_server;
pub mod session_store;
pub mod weixin;

use std::collections::{BTreeSet, HashMap};
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;

use self::app_server::{AppServerConfig, AppServerTurnResult, CodexAppServer};
use self::session_store::{ConnectSessionStore, ConnectState, PairingRequest, ReplyOutbox};
use self::weixin::{
    WeixinClient, WeixinLongPollErrorKind, WeixinMessage, classify_weixin_long_poll_error,
    split_reply_text,
};

pub const DEFAULT_WEIXIN_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const MAX_INBOUND_MESSAGE_AGE_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WeixinConnectConfig {
    #[serde(default = "default_weixin_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub allow_from: String,
    #[serde(default)]
    pub route_tag: String,
    #[serde(default)]
    pub work_dir: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_sandbox")]
    pub sandbox: String,
    #[serde(default)]
    pub codex_path: String,
}

impl Default for WeixinConnectConfig {
    fn default() -> Self {
        Self {
            base_url: default_weixin_base_url(),
            token: String::new(),
            account_id: String::new(),
            allow_from: String::new(),
            route_tag: String::new(),
            work_dir: String::new(),
            model: String::new(),
            sandbox: default_sandbox(),
            codex_path: String::new(),
        }
    }
}

impl WeixinConnectConfig {
    pub fn normalized(mut self) -> Self {
        self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
        if self.base_url.is_empty() {
            self.base_url = default_weixin_base_url();
        }
        self.token = self.token.trim().to_string();
        self.account_id = self.account_id.trim().to_string();
        self.allow_from = self.allow_from.trim().to_string();
        self.route_tag = self.route_tag.trim().to_string();
        self.work_dir = self.work_dir.trim().to_string();
        self.model = self.model.trim().to_string();
        self.sandbox = normalize_sandbox(&self.sandbox);
        self.codex_path = self.codex_path.trim().to_string();
        self
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WeixinConnectStatus {
    pub state: String,
    pub message: String,
    pub account_id: String,
    pub has_token: bool,
    pub last_peer_id: String,
    pub last_message_at_ms: u64,
    pub processed_messages: u64,
    pub weixin_state: String,
    pub codex_state: String,
    pub runtime_active: bool,
    pub recent_error: String,
    pub pending_messages: usize,
    pub failed_messages: usize,
}

impl Default for WeixinConnectStatus {
    fn default() -> Self {
        Self {
            state: "stopped".to_string(),
            message: "微信连接未启动。".to_string(),
            account_id: String::new(),
            has_token: false,
            last_peer_id: String::new(),
            last_message_at_ms: 0,
            processed_messages: 0,
            weixin_state: "stopped".to_string(),
            codex_state: "stopped".to_string(),
            runtime_active: false,
            recent_error: String::new(),
            pending_messages: 0,
            failed_messages: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeixinMessageSummary {
    pub key: String,
    pub peer_id: String,
    pub state: String,
    pub error: String,
    pub received_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeixinInbox {
    pub messages: Vec<WeixinMessageSummary>,
    pub pending_pairings: Vec<PairingRequest>,
    pub approved_peers: Vec<String>,
}

pub fn inbox_for_account(account_id: &str) -> anyhow::Result<WeixinInbox> {
    let state = ConnectSessionStore::default_for_account(account_id).load()?;
    Ok(WeixinInbox {
        messages: state
            .messages
            .iter()
            .map(|item| WeixinMessageSummary {
                key: item.key.clone(),
                peer_id: item.message.from_user_id.clone(),
                state: item.state.clone(),
                error: item.error.clone(),
                received_at_ms: item.received_at_ms,
            })
            .collect(),
        pending_pairings: state.pending_pairings.into_values().collect(),
        approved_peers: state.approved_peers.into_iter().collect(),
    })
}

pub type SharedWeixinConnectStatus = Arc<Mutex<WeixinConnectStatus>>;

fn state_change_notification() -> &'static tokio::sync::Notify {
    static NOTIFY: OnceLock<tokio::sync::Notify> = OnceLock::new();
    NOTIFY.get_or_init(tokio::sync::Notify::new)
}

pub fn notify_weixin_state_changed() {
    state_change_notification().notify_one();
}

/// 只让新消息切换 CLI，不中断已经执行的回合。
#[derive(Clone)]
pub struct WeixinCodexPath(Arc<Mutex<String>>);

impl WeixinCodexPath {
    pub fn new(path: &str) -> Self {
        Self(Arc::new(Mutex::new(path.trim().to_string())))
    }

    pub fn set(&self, path: &str) {
        *self.0.lock().unwrap_or_else(|error| error.into_inner()) = path.trim().to_string();
    }

    fn apply(&self, config: &mut AppServerConfig) -> bool {
        let path = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if config.executable == *path {
            return false;
        }
        config.executable.clone_from(&path);
        true
    }

    fn matches(&self, path: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_str()
            == path
    }
}

pub async fn run_weixin_connect(
    config: WeixinConnectConfig,
    stop: Arc<AtomicBool>,
    status: SharedWeixinConnectStatus,
) -> anyhow::Result<()> {
    let codex_path = WeixinCodexPath::new(&config.codex_path);
    run_weixin_connect_with_codex_path(config, stop, status, codex_path).await
}

pub async fn run_weixin_connect_with_codex_path(
    config: WeixinConnectConfig,
    stop: Arc<AtomicBool>,
    status: SharedWeixinConnectStatus,
    codex_path: WeixinCodexPath,
) -> anyhow::Result<()> {
    let config = config.normalized();
    if config.token.is_empty() {
        bail!("请先扫码登录微信");
    }
    let work_dir = if config.work_dir.is_empty() {
        std::env::current_dir().context("无法读取当前工作目录")?
    } else {
        PathBuf::from(&config.work_dir)
    };
    if !work_dir.is_dir() {
        bail!("工作目录不存在：{}", work_dir.display());
    }

    update_status(&status, |current| {
        current.state = "starting".to_string();
        current.weixin_state = "connecting".to_string();
        current.codex_state = "checking".to_string();
        current.runtime_active = true;
        current.message = "正在检查 Codex 并连接微信...".to_string();
        current.account_id = config.account_id.clone();
        current.has_token = true;
        current.recent_error.clear();
    });

    let client = WeixinClient::new(&config.base_url, &config.token, &config.route_tag)?;
    let store = ConnectSessionStore::default_for_account(&config.account_id);

    store.update(ConnectState::recover_interrupted)?;
    let mut app_config = AppServerConfig {
        executable: config.codex_path.clone(),
        work_dir,
        model: config.model.clone(),
        sandbox: config.sandbox.clone(),
    };
    let server_pool = Arc::new(tokio::sync::Mutex::new(Vec::<CodexAppServer>::new()));
    update_status(&status, |current| {
        current.codex_state = "ready".to_string();
        current.message = "Codex 已就绪，正在连接微信...".to_string();
    });

    let mut jobs = tokio::task::JoinSet::new();
    let mut active_peers = BTreeSet::new();
    let mut active_jobs = HashMap::new();
    let mut long_poll_timeout_ms = 35_000;
    let mut consecutive_errors = 0_u32;
    let mut cursor_reset_attempted = false;
    while !stop.load(Ordering::SeqCst) {
        while jobs.len() < 2 {
            let next = store.update(|state| {
                state.expire_old_pending_messages(now_ms(), MAX_INBOUND_MESSAGE_AGE_MS);
                state.claim_next(&active_peers)
            })?;
            let Some((key, message, prior_thread, existing_outbox)) = next else {
                break;
            };
            let peer = message.from_user_id.clone();
            active_peers.insert(peer.clone());
            if codex_path.apply(&mut app_config) {
                let mut pooled = server_pool.lock().await;
                for mut server in pooled.drain(..) {
                    server.close().await;
                }
            }
            let client = client.clone();
            let app_config = app_config.clone();
            let server_pool = Arc::clone(&server_pool);
            let task_stop = Arc::clone(&stop);
            let task_store = store.clone();
            let task_codex_path = codex_path.clone();
            let job_key = key.clone();
            let job_peer = peer.clone();
            let job = jobs.spawn(async move {
                let result = if existing_outbox.is_some() {
                    deliver_weixin_reply(&client, &task_store, &key, &peer, &app_config.work_dir)
                        .await
                } else {
                    let server = server_pool.lock().await.pop();
                    let mut server = match server {
                        Some(server) => server,
                        None => match CodexAppServer::start(app_config.clone()).await {
                            Ok(server) => server,
                            Err(error) => return (key, peer, Err(error)),
                        },
                    };
                    let result = async {
                        let Some(outbox) = create_weixin_reply_outbox(
                            &client,
                            &app_config,
                            &mut server,
                            &message,
                            prior_thread.as_deref(),
                            &task_stop,
                        )
                        .await?
                        else {
                            return Ok(prior_thread.clone().unwrap_or_default());
                        };
                        let saved =
                            task_store.update(|state| state.save_reply_outbox(&key, outbox))?;
                        anyhow::ensure!(saved, "无法保存微信回复队列；消息未发送");
                        deliver_weixin_reply(
                            &client,
                            &task_store,
                            &key,
                            &peer,
                            &app_config.work_dir,
                        )
                        .await
                    }
                    .await;
                    if result.is_ok()
                        && server.is_running()
                        && !task_stop.load(Ordering::SeqCst)
                        && task_codex_path.matches(&app_config.executable)
                    {
                        server_pool.lock().await.push(server);
                    } else {
                        server.close().await;
                    }
                    result
                };
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    client.send_typing(&message.from_user_id, &message.context_token, false),
                )
                .await;
                (key, peer, result)
            });
            active_jobs.insert(job.id(), (job_key, job_peer));
        }
        let state = store.load()?;
        update_status(&status, |current| {
            current.pending_messages = state.pending_count();
            current.failed_messages = state.failed_count();
        });
        tokio::select! {
            _ = wait_for_stop(&stop) => break,
            _ = state_change_notification().notified() => {},
            completed = jobs.join_next_with_id(), if !jobs.is_empty() => {
                match completed {
                    Some(Ok((id, (key, peer, result)))) => {
                        active_jobs.remove(&id);
                        active_peers.remove(&peer);
                        let (pending, failed) = store.update(|state| {
                            if let Some(item) = state.messages.iter_mut().find(|item| item.key == key) {
                                match &result {
                                    Ok(thread_id) => {
                                        item.state = "completed".to_string();
                                        item.error.clear();
                                        if !thread_id.trim().is_empty() {
                                            state.thread_ids.insert(peer.clone(), thread_id.clone());
                                        }
                                        state.mark_processed(&key);
                                        state.trim_completed();
                                    }
                                    Err(error) => {
                                        item.state = "failed".to_string();
                                        item.error = error.to_string();
                                    }
                                }
                            }
                            (state.pending_count(), state.failed_count())
                        })?;
                        update_status(&status, |current| {
                            current.pending_messages = pending;
                            current.failed_messages = failed;
                            current.last_peer_id = peer;
                            current.last_message_at_ms = now_ms();
                            match result {
                                Ok(_) => {
                                    current.processed_messages += 1;
                                    current.codex_state = "ready".to_string();
                                    if current.weixin_state == "connected" {
                                        current.recent_error.clear();
                                    }
                                    current.message = "最近一条微信消息已处理。".to_string();
                                }
                                Err(error) => {
                                    current.codex_state = "error".to_string();
                                    current.recent_error = format!("处理微信消息失败：{error}");
                                    current.message = current.recent_error.clone();
                                }
                            }
                        });
                    }
                    Some(Err(error)) => {
                        let id = error.id();
                        if let Some((key, peer)) = active_jobs.remove(&id) {
                            active_peers.remove(&peer);
                            let message = "微信消息处理任务异常终止，执行结果不明；请确认后手动重试。";
                            let (pending, failed) = store.update(|state| {
                                if let Some(item) = state.messages.iter_mut().find(|item| item.key == key) {
                                    item.state = "failed".to_string();
                                    item.error = message.to_string();
                                }
                                (state.pending_count(), state.failed_count())
                            })?;
                            update_status(&status, |current| {
                                current.pending_messages = pending;
                                current.failed_messages = failed;
                                current.last_peer_id = peer;
                                current.codex_state = "error".to_string();
                                current.recent_error = message.to_string();
                                current.message = current.recent_error.clone();
                            });
                        }
                    }
                    None => {}
                }
            }
            updates = client.get_updates(&state.get_updates_buf, long_poll_timeout_ms) => {
                match updates {
                    Ok(updates) => {
                        consecutive_errors = 0;
                        cursor_reset_attempted = false;
                        if updates.longpolling_timeout_ms > 0 {
                            long_poll_timeout_ms = updates.longpolling_timeout_ms;
                        }
                        let mut pairing_notices = Vec::new();
                        let mut ignored_peer = None;
                        let (pending, failed) = store.update(|state| {
                            for message in updates.messages {
                                if !message.is_finished_user_message()
                                    || !message.room_id.is_empty()
                                    || !message.chat_room_id.is_empty()
                                    || state.is_processed(&message.dedup_key())
                                { continue; }
                                if message.is_older_than(now_ms(), MAX_INBOUND_MESSAGE_AGE_MS) {
                                    state.mark_processed(message.dedup_key());
                                    continue;
                                }
                                let peer = message.from_user_id.clone();
                                if !is_allowed_peer(&config.allow_from, &peer)
                                    && !state.approved_peers.contains(&peer)
                                {
                                    ignored_peer = Some(peer.clone());
                                    if !state.pending_pairings.contains_key(&peer) {
                                        state.pending_pairings.insert(peer.clone(), PairingRequest {
                                            peer_id: peer.clone(), requested_at_ms: now_ms(),
                                        });
                                        pairing_notices.push((peer, message.context_token.clone()));
                                    }
                                    state.mark_processed(message.dedup_key());
                                    continue;
                                }
                                if !message.context_token.trim().is_empty() {
                                    state.set_latest_context_token(&peer, &message.context_token);
                                    state.retry_messages_waiting_for_context_token(&peer);
                                }
                                state.enqueue(message, now_ms());
                            }
                            if !updates.get_updates_buf.is_empty() {
                                state.get_updates_buf = updates.get_updates_buf;
                            }
                            (state.pending_count(), state.failed_count())
                        })?;
                        update_status(&status, |current| {
                            current.weixin_state = "connected".to_string();
                            current.state = "running".to_string();
                            current.pending_messages = pending;
                            current.failed_messages = failed;
                            if current.codex_state == "ready" {
                                current.recent_error.clear();
                                if let Some(peer) = ignored_peer {
                                    current.message = format!("已忽略来自非白名单发送方的消息：{peer}，请在 Manager 审批配对。");
                                } else if !current.message.starts_with("已忽略来自非白名单发送方的消息") {
                                    current.message = "微信已连接，Codex 可以回复。".to_string();
                                }
                            }
                        });
                        for (peer, token) in pairing_notices {
                            if !token.is_empty() {
                                let _ = client.send_text_chunks(&peer, "已收到配对请求，请在 Codex++ Manager 中批准后再发送消息。", &token).await;
                            }
                        }
                    }
                    Err(error) => {
                        let error_kind = if is_weixin_cursor_rejected(&error) {
                            WeixinLongPollErrorKind::CursorRejected
                        } else {
                            classify_weixin_long_poll_error(&error)
                        };
                        match error_kind {
                            WeixinLongPollErrorKind::CursorRejected if !cursor_reset_attempted => {
                                cursor_reset_attempted = true;
                                store.update(|state| state.get_updates_buf.clear())?;
                                update_status(&status, |current| {
                                    current.state = "retrying".to_string();
                                    current.weixin_state = "retrying".to_string();
                                    current.recent_error =
                                        "微信同步游标已失效，正在清除游标并重新连接。".to_string();
                                    current.message = current.recent_error.clone();
                                });
                                continue;
                            }
                            WeixinLongPollErrorKind::CursorRejected
                            | WeixinLongPollErrorKind::AuthenticationRejected => {
                                update_status(&status, |current| {
                                    current.state = "error".to_string();
                                    current.weixin_state = "needs_login".to_string();
                                    current.recent_error = "微信登录已失效，请重新扫码。".to_string();
                                    current.message = current.recent_error.clone();
                                    current.runtime_active = false;
                                });
                                bail!("微信登录已失效，请重新扫码：{error}");
                            }
                            WeixinLongPollErrorKind::Terminal => {
                                update_status(&status, |current| {
                                    current.state = "error".to_string();
                                    current.weixin_state = "error".to_string();
                                    current.recent_error = format!("微信长轮询遇到不可恢复错误：{error}");
                                    current.message = current.recent_error.clone();
                                    current.runtime_active = false;
                                });
                                bail!("微信长轮询遇到不可恢复错误：{error:#}");
                            }
                            WeixinLongPollErrorKind::Retryable => {
                                consecutive_errors = consecutive_errors.saturating_add(1);
                                update_status(&status, |current| {
                                    current.state = "retrying".to_string();
                                    current.weixin_state = "retrying".to_string();
                                    current.recent_error = format!("微信长轮询失败：{error}");
                                    current.message = current.recent_error.clone();
                                });
                                let backoff = weixin_reconnect_backoff(consecutive_errors);
                                tokio::select! {
                                    _ = wait_for_stop(&stop) => break,
                                    _ = tokio::time::sleep(backoff) => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    jobs.abort_all();
    update_status(&status, |current| {
        current.state = "stopped".to_string();
        current.weixin_state = "stopped".to_string();
        current.codex_state = "stopped".to_string();
        current.runtime_active = false;
        current.message = "微信连接已停止。".to_string();
    });
    Ok(())
}

async fn wait_for_stop(stop: &AtomicBool) {
    while !stop.load(Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn resolve_app_server(
    config: &WeixinConnectConfig,
    work_dir: PathBuf,
) -> anyhow::Result<(AppServerConfig, CodexAppServer)> {
    let mut candidates = Vec::new();
    if !config.codex_path.is_empty() {
        candidates.push(config.codex_path.clone());
        candidates.extend(replacement_standalone_codex_clis(&config.codex_path));
    } else {
        if let Some(path) = std::env::var_os("PATH") {
            for directory in std::env::split_paths(&path) {
                let executable = directory.join(if cfg!(windows) { "codex.exe" } else { "codex" });
                if executable.is_file() {
                    candidates.push(executable.to_string_lossy().into_owned());
                }
            }
        }
        candidates.push("codex".to_string());
        if let Some(app_dir) = crate::app_paths::resolve_codex_app_dir(None)
            && let Some(path) = crate::app_paths::find_bundled_codex_cli(&app_dir)
        {
            candidates.push(path.to_string_lossy().into_owned());
        }
    }
    let mut last_error = None;
    for executable in candidates {
        let candidate = AppServerConfig {
            executable,
            work_dir: work_dir.clone(),
            model: config.model.clone(),
            sandbox: config.sandbox.clone(),
        };
        match tokio::time::timeout(
            std::time::Duration::from_secs(20),
            CodexAppServer::start(candidate.clone()),
        )
        .await
        {
            Ok(Ok(server)) => return Ok((candidate, server)),
            Ok(Err(error)) => last_error = Some(error.to_string()),
            Err(_) => {
                last_error = Some(format!(
                    "Codex app-server 预检超时：{}",
                    candidate.executable
                ))
            }
        }
    }
    bail!(
        "Codex CLI 预检失败：{}",
        last_error.unwrap_or_else(|| "没有可用的 CLI".to_string())
    )
}

#[cfg(windows)]
fn replacement_standalone_codex_clis(configured: &str) -> Vec<String> {
    let path = Path::new(configured);
    if path.is_file()
        || !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("codex.exe"))
    {
        return Vec::new();
    }
    let Some(bin_dir) = path.parent().and_then(Path::parent) else {
        return Vec::new();
    };
    let Some(local_appdata) = std::env::var_os("LOCALAPPDATA") else {
        return Vec::new();
    };
    let expected = PathBuf::from(local_appdata)
        .join("OpenAI")
        .join("Codex")
        .join("bin");
    if !bin_dir
        .to_string_lossy()
        .eq_ignore_ascii_case(&expected.to_string_lossy())
    {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(bin_dir) else {
        return Vec::new();
    };
    let mut replacements = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("codex.exe"))
        .filter(|candidate| candidate.is_file())
        .map(|candidate| {
            let modified = std::fs::metadata(&candidate)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (modified, candidate)
        })
        .collect::<Vec<_>>();
    replacements.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    replacements
        .into_iter()
        .map(|(_, path)| path.to_string_lossy().into_owned())
        .collect()
}

#[cfg(not(windows))]
fn replacement_standalone_codex_clis(_: &str) -> Vec<String> {
    Vec::new()
}

pub async fn validate_codex_cli(
    executable: &str,
    work_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let candidate = AppServerConfig {
        executable: executable.to_string(),
        work_dir: work_dir.to_path_buf(),
        model: String::new(),
        sandbox: "read-only".to_string(),
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(20),
        CodexAppServer::start(candidate),
    )
    .await
    {
        Ok(Ok(mut server)) => {
            server.close().await;
            Ok(())
        }
        Ok(Err(error)) => Err(error),
        Err(_) => bail!("Codex app-server 预检超时：{executable}"),
    }
}

pub async fn list_codex_models(work_dir: &str, codex_path: &str) -> anyhow::Result<Vec<String>> {
    let work_dir = if work_dir.trim().is_empty() {
        std::env::current_dir().context("无法读取当前工作目录")?
    } else {
        PathBuf::from(work_dir.trim())
    };
    if !work_dir.is_dir() {
        bail!("工作目录不存在：{}", work_dir.display());
    }
    let config = WeixinConnectConfig {
        codex_path: codex_path.trim().to_string(),
        ..WeixinConnectConfig::default()
    };
    let (_, mut server) = resolve_app_server(&config, work_dir).await?;
    let models = server.list_models().await;
    server.close().await;
    models
}

/// 使用与微信消息相同的 app-server 路径实际执行一个短回合。
/// model/list 只能证明模型已配置，不能证明当前上游可以回复。
pub async fn probe_codex_model(
    work_dir: &str,
    codex_path: &str,
    model: &str,
) -> anyhow::Result<()> {
    let model = model.trim();
    if model.is_empty() {
        bail!("请先选择要测试的模型");
    }
    let work_dir = if work_dir.trim().is_empty() {
        std::env::current_dir().context("无法读取当前工作目录")?
    } else {
        PathBuf::from(work_dir.trim())
    };
    if !work_dir.is_dir() {
        bail!("工作目录不存在：{}", work_dir.display());
    }
    let config = WeixinConnectConfig {
        codex_path: codex_path.trim().to_string(),
        model: model.to_string(),
        sandbox: "read-only".to_string(),
        ..WeixinConnectConfig::default()
    };
    let (_, mut server) = resolve_app_server(&config, work_dir).await?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        let models = server.list_models().await?;
        if !models.iter().any(|available| available == model) {
            bail!("模型「{model}」不在当前 Codex 模型列表中");
        }
        let thread_id = server.prepare_thread(None).await?;
        let turn = server
            .run_turn(&thread_id, "请只回复 OK。不要使用工具。 ")
            .await?;
        if turn.reply.trim().is_empty() {
            bail!("模型完成了回合，但没有返回文字");
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;
    server.close().await;
    result.context("模型实际回复测试超过 90 秒")?
}

async fn create_weixin_reply_outbox(
    client: &WeixinClient,
    app_config: &AppServerConfig,
    server: &mut CodexAppServer,
    message: &WeixinMessage,
    saved_thread_id: Option<&str>,
    stop: &AtomicBool,
) -> anyhow::Result<Option<ReplyOutbox>> {
    let text = message.text().unwrap_or_default();
    if message.item_list.iter().any(|item| item.item_type == 5) {
        return Ok(Some(reply_outbox(
            saved_thread_id.unwrap_or_default(),
            "暂不支持视频，请发送图片、文件或文字。",
            Vec::new(),
        )));
    }
    if text.is_empty() && message.item_list.iter().any(|item| item.item_type == 3) {
        return Ok(Some(reply_outbox(
            saved_thread_id.unwrap_or_default(),
            "这条语音没有可用的微信转写文字，请改用文字发送。",
            Vec::new(),
        )));
    }
    if text.is_empty()
        && !message
            .item_list
            .iter()
            .any(|item| item.item_type == 2 || item.item_type == 4)
    {
        return Ok(None);
    }
    if let Some(reason) = local_model_endpoint_unreachable_reason().await {
        bail!("{reason}");
    }
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        client.send_typing(&message.from_user_id, &message.context_token, true),
    )
    .await;
    let mut prompt = if text.is_empty() {
        "请处理收到的微信附件。".to_string()
    } else {
        text
    };
    let mut input_images = Vec::new();
    for item in &message.item_list {
        let (media, image_key, suffix) = match item.item_type {
            2 => (
                item.image_item.as_ref(),
                item.image_item
                    .as_ref()
                    .and_then(|value| value["aeskey"].as_str()),
                ".jpg",
            ),
            4 => (item.file_item.as_ref(), None, ".bin"),
            _ => continue,
        };
        let media = media.context("微信附件缺少媒体信息")?;
        let bytes = client.download_media(media, image_key).await?;
        let suffix = if item.item_type == 2 {
            image_extension(&bytes)
        } else {
            media_file_extension(media["file_name"].as_str()).unwrap_or(suffix)
        };
        let path = save_inbound_media(&bytes, suffix)?;
        if item.item_type == 2 {
            input_images.push(json!({ "type": "localImage", "path": path.to_string_lossy() }));
        } else {
            let name = media["file_name"].as_str().unwrap_or("微信文件");
            prompt.push_str(&format!(
                "\n\n收到文件 {name}，本地路径：{}",
                path.display()
            ));
        }
    }
    prompt.push_str("\n\n如果需要把已生成的工作目录内文件作为微信附件发送，请在回复中单独写一行 [[WEIXIN_FILE:相对路径]]。只在用户明确要求发送文件时使用。 ");
    let thread_id = match server.prepare_thread(saved_thread_id).await {
        Ok(thread_id) => thread_id,
        Err(error) if saved_thread_id.is_some() => server
            .prepare_thread(None)
            .await
            .with_context(|| format!("恢复原会话失败（{error}），新建会话也失败"))?,
        Err(error) => return Err(error),
    };
    let turn_result = {
        let mut inputs = vec![json!({ "type": "text", "text": prompt, "text_elements": [] })];
        inputs.extend(input_images);
        let turn = server.run_turn_items(&thread_id, inputs);
        tokio::pin!(turn);
        loop {
            tokio::select! {
                reply = &mut turn => break reply?,
                _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                    if stop.load(Ordering::SeqCst) {
                        bail!("微信连接已停止");
                    }
                }
            }
        }
    };
    let (reply_text, attachments) = extract_attachments(&turn_result.reply, &app_config.work_dir)?;
    let reply = if reply_text.trim().is_empty() {
        "Codex 已完成处理，但没有返回文字内容。"
    } else {
        reply_text.trim()
    };
    let reply_with_footer = append_reply_footer(reply, &turn_result, &app_config.work_dir);
    Ok(Some(reply_outbox(
        &thread_id,
        &reply_with_footer,
        attachments
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
    )))
}

fn reply_outbox(thread_id: &str, text: &str, attachments: Vec<String>) -> ReplyOutbox {
    ReplyOutbox {
        thread_id: thread_id.to_string(),
        text_chunks: split_reply_text(text),
        attachments,
        delivered_text_chunks: 0,
        delivered_attachments: 0,
    }
}

async fn deliver_weixin_reply(
    client: &WeixinClient,
    store: &ConnectSessionStore,
    message_key: &str,
    peer_id: &str,
    work_dir: &std::path::Path,
) -> anyhow::Result<String> {
    loop {
        let state = store.load()?;
        let (outbox, context_token) = state
            .reply_outbox_for_delivery(message_key)
            .map(|(outbox, token)| (outbox.clone(), token.to_string()))
            .context("微信回复队列或最新 context_token 不存在")?;

        if let Some((index, text)) = outbox.next_text_chunk() {
            client
                .send_text_chunks(peer_id, text, &context_token)
                .await?;
            let advanced =
                store.update(|state| state.mark_reply_text_chunk_delivered(message_key, index))?;
            anyhow::ensure!(advanced, "微信回复已发送，但无法保存文本分块进度");
            continue;
        }

        if let Some((index, raw_path)) = outbox.next_attachment() {
            let root = work_dir
                .canonicalize()
                .context("微信回复工作目录不可读取")?;
            let candidate = PathBuf::from(raw_path);
            let candidate = if candidate.is_absolute() {
                candidate
            } else {
                root.join(candidate)
            };
            let path = candidate.canonicalize().context("微信回复附件已不存在")?;
            anyhow::ensure!(
                path.starts_with(&root) && path.is_file(),
                "微信回复附件必须是工作目录内的文件"
            );
            client.send_file(peer_id, &context_token, &path).await?;
            let advanced =
                store.update(|state| state.mark_reply_attachment_delivered(message_key, index))?;
            anyhow::ensure!(advanced, "微信附件已发送，但无法保存附件进度");
            continue;
        }

        anyhow::ensure!(outbox.is_delivered(), "微信回复队列进度无效");
        return Ok(outbox.thread_id);
    }
}

fn image_extension(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG") {
        ".png"
    } else if bytes.starts_with(b"GIF8") {
        ".gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        ".webp"
    } else {
        ".jpg"
    }
}

fn media_file_extension(name: Option<&str>) -> Option<&str> {
    let name = name?;
    let extension = name.rsplit_once('.')?.1;
    if extension.is_empty()
        || extension.len() > 10
        || !extension
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return None;
    }
    name.rfind('.').map(|index| &name[index..])
}

fn save_inbound_media(bytes: &[u8], suffix: &str) -> anyhow::Result<PathBuf> {
    let dir = crate::paths::default_app_state_dir().join("weixin-media");
    std::fs::create_dir_all(&dir).context("创建微信媒体目录失败")?;
    let path = dir.join(format!("{}{}", uuid::Uuid::new_v4(), suffix));
    std::fs::write(&path, bytes).context("保存微信媒体失败")?;
    Ok(path)
}

fn extract_attachments(
    reply: &str,
    work_dir: &std::path::Path,
) -> anyhow::Result<(String, Vec<PathBuf>)> {
    let root = work_dir.canonicalize().context("工作目录不可读取")?;
    let mut text = Vec::new();
    let mut attachments = Vec::new();
    let mut in_code_block = false;
    for line in reply.lines() {
        let marker = line.trim();
        if marker.starts_with("```") {
            in_code_block = !in_code_block;
            text.push(line);
            continue;
        }
        if let Some(raw) = (!in_code_block)
            .then_some(marker)
            .and_then(|marker| marker.strip_prefix("[[WEIXIN_FILE:"))
            .and_then(|value| value.strip_suffix("]]"))
        {
            if attachments.len() >= 4 {
                bail!("单条回复最多发送 4 个附件");
            }
            let candidate = std::path::Path::new(raw.trim());
            let path = if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                root.join(candidate)
            };
            let path = path.canonicalize().context("声明的微信附件不存在")?;
            if !path.starts_with(&root) || !path.is_file() {
                bail!("微信附件必须是工作目录内的文件");
            }
            if matches!(
                path.extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .as_str(),
                "mp4" | "mov" | "avi" | "mkv" | "webm" | "3gp"
            ) {
                bail!("微信视频附件暂不支持");
            }
            attachments.push(path);
        } else {
            text.push(line);
        }
    }
    Ok((text.join("\n"), attachments))
}

fn append_reply_footer(
    reply: &str,
    turn: &AppServerTurnResult,
    work_dir: &std::path::Path,
) -> String {
    let model = if turn.model.trim().is_empty() {
        "Codex"
    } else {
        turn.model.trim()
    };
    let context = match (turn.usage.context_used, turn.usage.context_window) {
        (Some(used), Some(window)) if window > 0 => {
            let percent = ((used as f64 / window as f64) * 100.0).round() as u64;
            format!("ctx {}%", percent.min(100))
        }
        _ => "ctx --".to_string(),
    };
    format!(
        "{}\n\n{} · {}\n{}",
        reply.trim(),
        model,
        context,
        compact_work_dir(work_dir)
    )
}

fn compact_work_dir(work_dir: &std::path::Path) -> String {
    let path = work_dir.to_string_lossy();
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::Path::new(&home).to_string_lossy();
        if path == home {
            return "~".to_string();
        }
        if let Some(relative) = path
            .strip_prefix(home.as_ref())
            .and_then(|value| value.strip_prefix('/').filter(|value| !value.is_empty()))
        {
            return format!("~/{relative}");
        }
    }
    path.into_owned()
}

fn is_allowed_peer(allow_from: &str, peer: &str) -> bool {
    let allow_from = allow_from.trim();
    allow_from == "*"
        || allow_from
            .split(',')
            .map(str::trim)
            .any(|allowed| !allowed.is_empty() && allowed == peer)
}

/// 模型流量若走本地代理（config.toml 的 openai_base_url 指向 127.0.0.1，
/// 由 Codex++ 的单模型路由写入），该代理只随从 Codex++ 启动的桌面版存在。
/// 提前探测，避免 turn 失败后只剩一句模糊的「处理失败」。
async fn local_model_endpoint_unreachable_reason() -> Option<String> {
    let base_url =
        std::fs::read_to_string(crate::relay_config::default_codex_home_dir().join("config.toml"))
            .ok()
            .and_then(|contents| {
                crate::relay_config::root_key_string(&contents, "openai_base_url")
            })?;
    let (host, port) = localhost_endpoint(&base_url)?;
    let connect = tokio::net::TcpStream::connect((host.as_str(), port));
    if tokio::time::timeout(std::time::Duration::from_millis(300), connect)
        .await
        .is_ok_and(|result| result.is_ok())
    {
        return None;
    }
    Some(
        "Codex 桌面版当前未运行，微信连接暂时无法调用模型。\
         请先从 Codex++ 启动 Codex 桌面版后重试。"
            .to_string(),
    )
}

/// 只对「本地回环 + 显式端口」的 base_url 预检；
/// 直连中转站或未带端口的地址不属于桌面版代理，返回 None 表示跳过。
fn localhost_endpoint(base_url: &str) -> Option<(String, u16)> {
    let url = reqwest::Url::parse(base_url.trim()).ok()?;
    let host = url.host_str()?;
    if host != "127.0.0.1" && host != "localhost" {
        return None;
    }
    Some((host.to_string(), url.port()?))
}

fn normalize_sandbox(value: &str) -> String {
    match value.trim() {
        "workspace-write" => "workspace-write",
        "danger-full-access" => "danger-full-access",
        _ => "read-only",
    }
    .to_string()
}

fn default_weixin_base_url() -> String {
    DEFAULT_WEIXIN_BASE_URL.to_string()
}

fn default_sandbox() -> String {
    "read-only".to_string()
}

fn update_status(
    status: &SharedWeixinConnectStatus,
    update: impl FnOnce(&mut WeixinConnectStatus),
) {
    if let Ok(mut current) = status.lock() {
        update(&mut current);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn is_weixin_cursor_rejected(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}");
    classify_weixin_long_poll_error(error) == WeixinLongPollErrorKind::CursorRejected
        || message.contains("ret=-14")
        || message.contains("errcode=-14")
}

fn weixin_reconnect_backoff_base_seconds(attempt: u32) -> u64 {
    let exponent = attempt.saturating_sub(1).min(5);
    (1_u64 << exponent).min(30)
}

fn weixin_reconnect_backoff(attempt: u32) -> std::time::Duration {
    let base_ms = weixin_reconnect_backoff_base_seconds(attempt) * 1_000;
    let jitter_cap_ms = 30_000_u64.saturating_sub(base_ms).min(500);
    let jitter_ms = if jitter_cap_ms == 0 {
        0
    } else {
        now_ms() % (jitter_cap_ms + 1)
    };
    std::time::Duration::from_millis(base_ms + jitter_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_cli_path_replaces_startup_path_at_next_message() {
        let path = WeixinCodexPath::new("missing-codex");
        let runtime_path = path.clone();
        let mut config = AppServerConfig {
            executable: "missing-codex".to_string(),
            work_dir: PathBuf::from("."),
            model: "test-model".to_string(),
            sandbox: "read-only".to_string(),
        };
        assert!(!runtime_path.apply(&mut config));
        path.set(" C:/new/codex.exe ");
        assert_eq!(config.executable, "missing-codex");
        assert!(runtime_path.apply(&mut config));
        assert_eq!(config.executable, "C:/new/codex.exe");
        assert_eq!(config.model, "test-model");
        assert!(!runtime_path.apply(&mut config));
        path.set("   ");
        assert!(runtime_path.apply(&mut config));
        assert_eq!(config.executable, "");
    }

    #[test]
    fn allow_from_supports_wildcard_and_comma_separated_ids() {
        assert!(!is_allowed_peer("", "a@im.wechat"));
        assert!(is_allowed_peer("*", "a@im.wechat"));
        assert!(is_allowed_peer("a@im.wechat, b@im.wechat", "b@im.wechat"));
        assert!(!is_allowed_peer("a@im.wechat", "b@im.wechat"));
    }

    #[test]
    fn reconnect_backoff_grows_to_a_30_second_cap() {
        let delays = (1..=8)
            .map(weixin_reconnect_backoff_base_seconds)
            .collect::<Vec<_>>();
        assert_eq!(delays, vec![1, 2, 4, 8, 16, 30, 30, 30]);
        assert!(weixin_reconnect_backoff(1).as_secs_f64() >= 1.0);
        assert!(weixin_reconnect_backoff(6) <= std::time::Duration::from_secs(30));
    }

    #[test]
    fn stale_cursor_rejection_is_distinguished_from_other_api_errors() {
        assert!(is_weixin_cursor_rejected(&anyhow::anyhow!(
            "微信长轮询被拒绝：ret=-14 errcode=0"
        )));
        assert!(is_weixin_cursor_rejected(&anyhow::anyhow!(
            "微信长轮询被拒绝：ret=0 errcode=-14"
        )));
        assert!(!is_weixin_cursor_rejected(&anyhow::anyhow!(
            "微信长轮询请求失败：连接重置"
        )));
    }

    /// 白名单外的消息不再无声消失：状态栏要能看到「已忽略」，
    /// 否则用户无法区分「消息没到」和「被过滤」（群内真实发生过）。
    /// 用 wiremock 假微信网关驱动真实消息循环验证。
    #[tokio::test]
    async fn non_allowlisted_message_updates_status_and_is_dropped() {
        let server = wiremock::MockServer::start().await;
        let message = serde_json::json!({
            "seq": 1,
            "message_id": (now_ms() % 1_000_000) as i64,
            "from_user_id": "stranger@im.wechat",
            "client_id": "unit-test",
            "create_time_ms": now_ms() as i64,
            "message_type": 1,
            "message_state": 2,
            "item_list": [{ "type": 1, "text_item": { "text": "hi" } }],
            "context_token": "ctx"
        });
        let updates = serde_json::json!({
            "ret": 0,
            "errcode": 0,
            "errmsg": "",
            "msgs": [message],
            "get_updates_buf": "YnVm",
            "longpolling_timeout_ms": 1000
        });
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/ilink/bot/getupdates"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(updates)
                    .set_delay(std::time::Duration::from_millis(300)),
            )
            .mount(&server)
            .await;

        let work_dir = tempfile::tempdir().unwrap();
        let config = WeixinConnectConfig {
            base_url: server.uri(),
            token: "tok".to_string(),
            account_id: "unit-test-ignore".to_string(),
            allow_from: "only-me@im.wechat".to_string(),
            route_tag: String::new(),
            work_dir: work_dir.path().display().to_string(),
            model: String::new(),
            sandbox: "read-only".to_string(),
            codex_path: String::new(),
        };
        let status: SharedWeixinConnectStatus = Arc::new(Mutex::new(Default::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(run_weixin_connect_with_codex_path(
            config,
            Arc::clone(&stop),
            Arc::clone(&status),
            WeixinCodexPath::new(""),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        // 循环正常退出时会写「微信连接已停止」，必须在停止前快照处理状态
        let snapshot = status
            .lock()
            .map(|current| current.message.clone())
            .unwrap_or_default();
        stop.store(true, Ordering::SeqCst);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;

        let message = snapshot;
        assert!(
            message.contains("已忽略来自非白名单发送方的消息"),
            "状态栏应提示已忽略，实际：{message}"
        );
        // 清理循环写入真实 app state 目录的测试状态文件
        let _ = std::fs::remove_file(
            crate::paths::default_app_state_dir()
                .join("weixin-connect-state-unit-test-ignore.json"),
        );
    }

    #[test]
    fn localhost_endpoint_only_matches_explicit_local_ports() {
        assert_eq!(
            localhost_endpoint("http://127.0.0.1:57321/v1"),
            Some(("127.0.0.1".to_string(), 57321))
        );
        assert_eq!(
            localhost_endpoint("http://localhost:8080/v1"),
            Some(("localhost".to_string(), 8080))
        );
        // 直连中转站不属于桌面版代理，不预检
        assert_eq!(localhost_endpoint("https://api.example.com/v1"), None);
        // 无显式端口的本地地址也不预检，避免误判成「桌面版未运行」
        assert_eq!(localhost_endpoint("http://127.0.0.1/v1"), None);
    }

    #[test]
    fn attachment_markers_require_files_inside_work_dir() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("report.txt"), b"ok").unwrap();
        let (text, files) =
            extract_attachments("已生成\n[[WEIXIN_FILE:report.txt]]", root.path()).unwrap();
        assert_eq!(text, "已生成");
        assert_eq!(files.len(), 1);
        assert!(extract_attachments("[[WEIXIN_FILE:../outside.txt]]", root.path()).is_err());
        let (example, files) =
            extract_attachments("```\n[[WEIXIN_FILE:report.txt]]\n```", root.path()).unwrap();
        assert!(example.contains("[[WEIXIN_FILE:report.txt]]"));
        assert!(files.is_empty());
    }

    #[test]
    fn config_normalizes_base_url_and_sandbox() {
        let config = WeixinConnectConfig {
            base_url: " https://example.test/ ".to_string(),
            sandbox: "unknown".to_string(),
            ..WeixinConnectConfig::default()
        }
        .normalized();
        assert_eq!(config.base_url, "https://example.test");
        assert_eq!(config.sandbox, "read-only");
    }

    #[test]
    fn reply_footer_contains_model_context_and_compact_work_dir() {
        let turn = AppServerTurnResult {
            reply: "完成了".to_string(),
            model: "gpt-5.5".to_string(),
            usage: app_server::TurnUsage {
                context_used: Some(41772),
                context_window: Some(1_000_000),
            },
        };
        let footer = append_reply_footer(
            "完成了",
            &turn,
            std::path::Path::new("/Users/tester/project"),
        );
        assert!(footer.contains("gpt-5.5 · ctx 4%"));
        assert!(footer.ends_with("/Users/tester/project"));
    }

    #[test]
    fn reply_footer_does_not_invent_context_when_usage_is_missing() {
        let turn = AppServerTurnResult {
            reply: "done".to_string(),
            model: String::new(),
            usage: app_server::TurnUsage::default(),
        };
        let footer = append_reply_footer("done", &turn, std::path::Path::new("/tmp/work"));
        assert!(footer.contains("Codex · ctx --"));
        assert!(footer.ends_with("/tmp/work"));
    }
}
