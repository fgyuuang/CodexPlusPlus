pub mod app_server;
pub mod session_store;
pub mod weixin;

use std::collections::BTreeSet;
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
use self::session_store::{ConnectSessionStore, ConnectState, PairingRequest};
use self::weixin::{WeixinClient, WeixinMessage};

pub const DEFAULT_WEIXIN_BASE_URL: &str = "https://ilinkai.weixin.qq.com";

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

pub async fn run_weixin_connect(
    config: WeixinConnectConfig,
    stop: Arc<AtomicBool>,
    status: SharedWeixinConnectStatus,
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
    let (app_config, mut initial_server) = resolve_app_server(&config, work_dir).await?;
    if !config.model.is_empty() {
        let models = initial_server
            .list_models()
            .await
            .context("读取 Codex 可用模型失败")?;
        if !models.iter().any(|model| model == &config.model) {
            bail!(
                "所选模型「{}」不在当前 Codex 模型列表中，请在 Manager 中重新选择模型并重启连接",
                config.model
            );
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(90), async {
        let thread_id = initial_server.prepare_thread(None).await?;
        let reply = initial_server
            .run_turn(&thread_id, "请只回复 OK。不要使用工具。")
            .await?;
        if reply.reply.trim().is_empty() {
            bail!("Codex 完成了预检回合，但没有返回文字");
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("Codex 模型回复预检超过 90 秒")??;
    let server_pool = Arc::new(tokio::sync::Mutex::new(vec![initial_server]));
    update_status(&status, |current| {
        current.codex_state = "ready".to_string();
        current.message = "Codex 已就绪，正在连接微信...".to_string();
    });

    let mut jobs = tokio::task::JoinSet::new();
    let mut active_peers = BTreeSet::new();
    let mut long_poll_timeout_ms = 35_000;
    let mut consecutive_errors = 0_u32;
    while !stop.load(Ordering::SeqCst) {
        while jobs.len() < 2 {
            let next = store.update(|state| state.claim_next(&active_peers))?;
            let Some((key, message, prior_thread)) = next else {
                break;
            };
            let peer = message.from_user_id.clone();
            active_peers.insert(peer.clone());
            let client = client.clone();
            let app_config = app_config.clone();
            let server_pool = Arc::clone(&server_pool);
            let task_stop = Arc::clone(&stop);
            jobs.spawn(async move {
                let server = server_pool.lock().await.pop();
                let mut server = match server {
                    Some(server) => server,
                    None => match CodexAppServer::start(app_config.clone()).await {
                        Ok(server) => server,
                        Err(error) => return (key, peer, Err(error)),
                    },
                };
                let result = process_weixin_message(
                    &client,
                    &app_config,
                    &mut server,
                    &message,
                    prior_thread.as_deref(),
                    &task_stop,
                )
                .await;
                if result.is_ok() && server.is_running() && !task_stop.load(Ordering::SeqCst) {
                    server_pool.lock().await.push(server);
                } else {
                    server.close().await;
                }
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    client.send_typing(&message.from_user_id, &message.context_token, false),
                )
                .await;
                (key, peer, result)
            });
        }
        let state = store.load()?;
        update_status(&status, |current| {
            current.pending_messages = state.pending_count();
            current.failed_messages = state.failed_count();
        });
        tokio::select! {
            _ = wait_for_stop(&stop) => break,
            _ = state_change_notification().notified() => {},
            completed = jobs.join_next(), if !jobs.is_empty() => {
                if let Some(Ok((key, peer, result))) = completed {
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
            }
            updates = client.get_updates(&state.get_updates_buf, long_poll_timeout_ms) => {
                match updates {
                    Ok(updates) => {
                        consecutive_errors = 0;
                        if updates.longpolling_timeout_ms > 0 {
                            long_poll_timeout_ms = updates.longpolling_timeout_ms;
                        }
                        let mut pairing_notices = Vec::new();
                        let (pending, failed) = store.update(|state| {
                            for message in updates.messages {
                                if !message.is_finished_user_message()
                                    || !message.room_id.is_empty()
                                    || !message.chat_room_id.is_empty()
                                    || state.is_processed(&message.dedup_key())
                                { continue; }
                                let peer = message.from_user_id.clone();
                                if !is_allowed_peer(&config.allow_from, &peer)
                                    && !state.approved_peers.contains(&peer)
                                {
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
                                    state.context_tokens.insert(peer, message.context_token.clone());
                                    state.enqueue(message, now_ms());
                                }
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
                                current.message = "微信已连接，Codex 可以回复。".to_string();
                            }
                        });
                        for (peer, token) in pairing_notices {
                            if !token.is_empty() {
                                let _ = client.send_text_chunks(&peer, "已收到配对请求，请在 Codex++ Manager 中批准后再发送消息。", &token).await;
                            }
                        }
                    }
                    Err(error) => {
                        if error.to_string().contains("errcode=-14") || error.to_string().contains("ret=-14") {
                            update_status(&status, |current| {
                                current.weixin_state = "needs_login".to_string();
                                current.recent_error = "微信登录已失效，请重新扫码。".to_string();
                            });
                            bail!("微信登录已失效，请重新扫码");
                        }
                        consecutive_errors = consecutive_errors.saturating_add(1);
                        update_status(&status, |current| {
                            current.state = "retrying".to_string();
                            current.weixin_state = "retrying".to_string();
                            current.recent_error = format!("微信长轮询失败：{error}");
                            current.message = current.recent_error.clone();
                        });
                        let backoff = 2_u64.pow(consecutive_errors.min(4)).min(30);
                        tokio::select! {
                            _ = wait_for_stop(&stop) => break,
                            _ = tokio::time::sleep(std::time::Duration::from_secs(backoff)) => {}
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

async fn process_weixin_message(
    client: &WeixinClient,
    app_config: &AppServerConfig,
    server: &mut CodexAppServer,
    message: &WeixinMessage,
    saved_thread_id: Option<&str>,
    stop: &AtomicBool,
) -> anyhow::Result<String> {
    let text = message.text().unwrap_or_default();
    if message.item_list.iter().any(|item| item.item_type == 5) {
        client
            .send_text_chunks(
                &message.from_user_id,
                "暂不支持视频，请发送图片、文件或文字。",
                &message.context_token,
            )
            .await?;
        return Ok(saved_thread_id.unwrap_or_default().to_string());
    }
    if text.is_empty() && message.item_list.iter().any(|item| item.item_type == 3) {
        client
            .send_text_chunks(
                &message.from_user_id,
                "这条语音没有可用的微信转写文字，请改用文字发送。",
                &message.context_token,
            )
            .await?;
        return Ok(saved_thread_id.unwrap_or_default().to_string());
    }
    if text.is_empty()
        && !message
            .item_list
            .iter()
            .any(|item| item.item_type == 2 || item.item_type == 4)
    {
        return Ok(saved_thread_id.unwrap_or_default().to_string());
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
    client
        .send_text_chunks(
            &message.from_user_id,
            &reply_with_footer,
            &message.context_token,
        )
        .await?;
    for path in attachments {
        client
            .send_file(&message.from_user_id, &message.context_token, &path)
            .await?;
    }
    Ok(thread_id)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_from_supports_wildcard_and_comma_separated_ids() {
        assert!(!is_allowed_peer("", "a@im.wechat"));
        assert!(is_allowed_peer("*", "a@im.wechat"));
        assert!(is_allowed_peer("a@im.wechat, b@im.wechat", "b@im.wechat"));
        assert!(!is_allowed_peer("a@im.wechat", "b@im.wechat"));
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
