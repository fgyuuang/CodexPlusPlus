use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use anyhow::Context;
use serde::{Deserialize, Serialize};

const STATE_FILE: &str = "weixin-connect-state.json";
const MAX_PROCESSED_IDS: usize = 512;
const MISSING_CONTEXT_TOKEN_ERROR: &str =
    "收到缺少 context_token 的微信消息；收到该联系人的新消息后可重试。";

use super::weixin::WeixinMessage;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueuedMessage {
    pub key: String,
    pub message: WeixinMessage,
    /// pending, processing, failed, or completed. Processing is failed on restart.
    pub state: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub received_at_ms: u64,
    /// 已生成但尚未确认完整送达的回复。旧队列文件没有此字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_outbox: Option<ReplyOutbox>,
}

/// A durable reply payload. Delivery counters advance only after the caller receives
/// a successful acknowledgement for the corresponding chunk or attachment.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReplyOutbox {
    #[serde(default)]
    pub thread_id: String,
    #[serde(default)]
    pub text_chunks: Vec<String>,
    #[serde(default)]
    pub attachments: Vec<String>,
    #[serde(default)]
    pub delivered_text_chunks: usize,
    #[serde(default)]
    pub delivered_attachments: usize,
}

impl ReplyOutbox {
    pub fn next_text_chunk(&self) -> Option<(usize, &str)> {
        let index = self.delivered_text_chunks;
        self.text_chunks
            .get(index)
            .map(|text| (index, text.as_str()))
    }

    pub fn mark_text_chunk_delivered(&mut self, index: usize) -> bool {
        if index != self.delivered_text_chunks || index >= self.text_chunks.len() {
            return false;
        }
        self.delivered_text_chunks += 1;
        true
    }

    pub fn next_attachment(&self) -> Option<(usize, &str)> {
        if self.delivered_text_chunks < self.text_chunks.len() {
            return None;
        }
        let index = self.delivered_attachments;
        self.attachments
            .get(index)
            .map(|path| (index, path.as_str()))
    }

    pub fn mark_attachment_delivered(&mut self, index: usize) -> bool {
        if self.delivered_text_chunks < self.text_chunks.len()
            || index != self.delivered_attachments
            || index >= self.attachments.len()
        {
            return false;
        }
        self.delivered_attachments += 1;
        true
    }

    pub fn is_delivered(&self) -> bool {
        self.delivered_text_chunks >= self.text_chunks.len()
            && self.delivered_attachments >= self.attachments.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairingRequest {
    pub peer_id: String,
    pub requested_at_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectState {
    #[serde(default)]
    pub get_updates_buf: String,
    #[serde(default)]
    pub thread_ids: BTreeMap<String, String>,
    #[serde(default)]
    pub completed_resets: BTreeMap<String, String>,
    #[serde(default)]
    pub context_tokens: BTreeMap<String, String>,
    #[serde(default)]
    pub messages: VecDeque<QueuedMessage>,
    #[serde(default)]
    pub approved_peers: BTreeSet<String>,
    #[serde(default)]
    pub pending_pairings: BTreeMap<String, PairingRequest>,
    #[serde(default)]
    processed_message_keys: VecDeque<String>,
}

impl ConnectState {
    pub fn claim_next(
        &mut self,
        active_peers: &BTreeSet<String>,
    ) -> Option<(String, WeixinMessage, Option<String>, Option<ReplyOutbox>)> {
        let index = self.messages.iter().enumerate().find_map(|(index, item)| {
            if item.state != "pending" || active_peers.contains(&item.message.from_user_id) {
                return None;
            }
            let blocked = self.messages.iter().take(index).any(|earlier| {
                earlier.message.from_user_id == item.message.from_user_id
                    && earlier.state != "completed"
            });
            (!blocked).then_some(index)
        })?;
        let item = &mut self.messages[index];
        item.state = "processing".to_string();
        let message = item.message.clone();
        let prior_thread = self.thread_ids.get(&message.from_user_id).cloned();
        Some((
            item.key.clone(),
            message,
            prior_thread,
            item.reply_outbox.clone(),
        ))
    }

    pub fn expire_old_pending_messages(&mut self, now_ms: u64, max_age_ms: u64) -> usize {
        let mut expired_keys = Vec::new();
        for item in &mut self.messages {
            if item.state == "pending" && item.message.is_older_than(now_ms, max_age_ms) {
                item.state = "completed".to_string();
                item.error.clear();
                expired_keys.push(item.key.clone());
            }
        }
        let expired_count = expired_keys.len();
        for key in expired_keys {
            self.mark_processed(key);
        }
        self.trim_completed();
        expired_count
    }

    pub fn enqueue(&mut self, message: WeixinMessage, now_ms: u64) -> bool {
        let key = message.dedup_key();
        if self.is_processed(&key) || self.messages.iter().any(|queued| queued.key == key) {
            return false;
        }
        let has_context_token = !message.context_token.trim().is_empty();
        if has_context_token {
            self.context_tokens
                .insert(message.from_user_id.clone(), message.context_token.clone());
        }
        self.messages.push_back(QueuedMessage {
            key,
            message,
            state: if has_context_token {
                "pending"
            } else {
                "failed"
            }
            .to_string(),
            error: if has_context_token {
                String::new()
            } else {
                MISSING_CONTEXT_TOKEN_ERROR.to_string()
            },
            received_at_ms: now_ms,
            reply_outbox: None,
        });
        true
    }

    /// Records a generated reply exactly once so a retry can resume delivery instead
    /// of running the model again. Existing progress must never be overwritten.
    pub fn save_reply_outbox(&mut self, message_key: &str, outbox: ReplyOutbox) -> bool {
        let Some(item) = self
            .messages
            .iter_mut()
            .find(|item| item.key == message_key)
        else {
            return false;
        };
        if item.reply_outbox.is_some() {
            return false;
        }
        item.reply_outbox = Some(outbox);
        true
    }

    /// Returns the saved outbox and the freshest known token for its peer.
    /// The original message token is a backward-compatible fallback for older state files.
    pub fn reply_outbox_for_delivery(&self, message_key: &str) -> Option<(&ReplyOutbox, &str)> {
        let item = self.messages.iter().find(|item| item.key == message_key)?;
        let outbox = item.reply_outbox.as_ref()?;
        let token = self
            .context_tokens
            .get(&item.message.from_user_id)
            .filter(|token| !token.trim().is_empty())
            .map(String::as_str)
            .or_else(|| {
                (!item.message.context_token.trim().is_empty())
                    .then_some(item.message.context_token.as_str())
            })?;
        Some((outbox, token))
    }

    pub fn set_latest_context_token(&mut self, peer_id: &str, context_token: &str) -> bool {
        let peer_id = peer_id.trim();
        let context_token = context_token.trim();
        if peer_id.is_empty() || context_token.is_empty() {
            return false;
        }
        self.context_tokens
            .insert(peer_id.to_string(), context_token.to_string());
        true
    }

    /// Requeues only messages that were never processed because their inbound
    /// update lacked a reply token. A new update from that peer makes them safe
    /// to run; other failed messages still require an explicit retry.
    pub fn retry_messages_waiting_for_context_token(&mut self, peer_id: &str) -> usize {
        let Some(context_token) = self
            .context_tokens
            .get(peer_id)
            .filter(|token| !token.trim().is_empty())
            .cloned()
        else {
            return 0;
        };
        let mut retried = 0;
        for item in &mut self.messages {
            if item.message.from_user_id == peer_id
                && item.state == "failed"
                && item.error == MISSING_CONTEXT_TOKEN_ERROR
                && item.reply_outbox.is_none()
            {
                item.message.context_token.clone_from(&context_token);
                item.state = "pending".to_string();
                item.error.clear();
                retried += 1;
            }
        }
        retried
    }

    pub fn retry_failed_message(&mut self, message_key: &str) -> bool {
        let Some(index) = self
            .messages
            .iter()
            .position(|item| item.key == message_key && item.state == "failed")
        else {
            return false;
        };
        let peer_id = self.messages[index].message.from_user_id.clone();
        let has_context_token = self
            .context_tokens
            .get(&peer_id)
            .is_some_and(|token| !token.trim().is_empty())
            || !self.messages[index].message.context_token.trim().is_empty();
        if !has_context_token {
            return false;
        }
        let item = &mut self.messages[index];
        if let Some(token) = self.context_tokens.get(&peer_id) {
            if !token.trim().is_empty() {
                item.message.context_token.clone_from(token);
            }
        }
        item.state = "pending".to_string();
        item.error.clear();
        true
    }

    pub fn mark_reply_text_chunk_delivered(&mut self, message_key: &str, index: usize) -> bool {
        self.messages
            .iter_mut()
            .find(|item| item.key == message_key)
            .and_then(|item| item.reply_outbox.as_mut())
            .is_some_and(|outbox| outbox.mark_text_chunk_delivered(index))
    }

    pub fn mark_reply_attachment_delivered(&mut self, message_key: &str, index: usize) -> bool {
        self.messages
            .iter_mut()
            .find(|item| item.key == message_key)
            .and_then(|item| item.reply_outbox.as_mut())
            .is_some_and(|outbox| outbox.mark_attachment_delivered(index))
    }

    pub fn recover_interrupted(&mut self) {
        for queued in &mut self.messages {
            if queued.state == "processing" {
                queued.state = "failed".to_string();
                queued.error = "上次处理被中断，执行结果不明；请确认后手动重试。".to_string();
            }
        }
    }

    pub fn pending_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|item| item.state == "pending")
            .count()
    }

    pub fn failed_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|item| item.state == "failed")
            .count()
    }

    pub fn trim_completed(&mut self) {
        let mut remove = self
            .messages
            .iter()
            .filter(|item| item.state == "completed")
            .count()
            .saturating_sub(128);
        self.messages.retain(|item| {
            if item.state == "completed" && remove > 0 {
                remove -= 1;
                false
            } else {
                true
            }
        });
    }

    pub fn is_processed(&self, message_key: &str) -> bool {
        !message_key.is_empty()
            && self
                .processed_message_keys
                .iter()
                .any(|key| key == message_key)
    }

    pub fn mark_processed(&mut self, message_key: impl Into<String>) {
        let message_key = message_key.into();
        if message_key.is_empty() || self.is_processed(&message_key) {
            return;
        }
        self.processed_message_keys.push_back(message_key);
        while self.processed_message_keys.len() > MAX_PROCESSED_IDS {
            self.processed_message_keys.pop_front();
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConnectSessionStore {
    path: PathBuf,
    published_path: Option<PathBuf>,
}

impl ConnectSessionStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            published_path: None,
        }
    }

    pub fn default_for_account(account_id: &str) -> Self {
        Self::in_directory(&crate::paths::default_app_state_dir(), account_id)
    }

    fn in_directory(directory: &std::path::Path, account_id: &str) -> Self {
        let account = account_id.trim();
        let file_name = if account.is_empty() {
            STATE_FILE.to_string()
        } else {
            format!("weixin-connect-state-{account}.json")
        };
        Self {
            path: directory
                .join("weixin-connect-state")
                .join(format!("{}.json", account_key(account))),
            published_path: (account.is_empty()
                || (sanitize_account_id(account) == account && account != "default"))
                .then(|| directory.join(file_name)),
        }
    }

    pub fn load(&self) -> anyhow::Result<ConnectState> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("无法解析微信连接状态 {}", self.path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(path) = &self.published_path {
                    match std::fs::read_dir(path.parent().context("状态目录不存在")?) {
                        Ok(entries) => {
                            for entry in entries {
                                let entry = entry?;
                                if Some(entry.file_name().as_os_str()) == path.file_name() {
                                    return serde_json::from_slice(&std::fs::read(entry.path())?)
                                        .context("无法解析已发布的微信连接状态");
                                }
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                Ok(ConnectState::default())
            }
            Err(error) => {
                Err(error).with_context(|| format!("无法读取微信连接状态 {}", self.path.display()))
            }
        }
    }

    pub fn save(&self, state: &ConnectState) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(state)?;
        crate::settings::atomic_write(&self.path, &bytes)
    }

    pub fn update<T>(&self, update: impl FnOnce(&mut ConnectState) -> T) -> anyhow::Result<T> {
        static STATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = STATE_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .map_err(|_| anyhow::anyhow!("微信状态锁已损坏"))?;
        let mut state = self.load()?;
        let result = update(&mut state);
        self.save(&state)?;
        Ok(result)
    }
}

pub(super) fn account_key(account: &str) -> String {
    let encoded: String = account
        .trim()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("account-{encoded}")
}

fn sanitize_account_id(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "default".to_string();
    }
    value
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '\0' => '_',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_paths_preserve_at_dot_and_uppercase_without_migrating_state() {
        let temp = tempfile::tempdir().unwrap();
        for account in ["Bot.ABC123@im.bot", "Sender.XYZ@im.wechat"] {
            let default = ConnectSessionStore::default_for_account(account);
            let filename = format!("weixin-connect-state-{account}.json");
            assert_eq!(
                default
                    .published_path
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_str(),
                Some(filename.as_str())
            );
            assert_eq!(sanitize_account_id(account), account);
            let store = ConnectSessionStore::new(temp.path().join(filename));
            let mut state = ConnectState::default();
            state.get_updates_buf = format!("cursor-{account}");
            state
                .thread_ids
                .insert("peer".into(), format!("thread-{account}"));
            state
                .context_tokens
                .insert("peer".into(), "saved-context".into());
            state.mark_processed("inbound-message");
            store.save(&state).unwrap();
            assert_eq!(store.load().unwrap(), state);
        }
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
        assert_eq!(sanitize_account_id("  "), "default");
        assert_eq!(sanitize_account_id("  Bot/A\\B:C\0  "), "Bot_A_B_C_");
    }

    #[test]
    fn empty_account_preserves_published_thread_and_prefers_new_state_after_save() {
        let temp = tempfile::tempdir().unwrap();
        let published = temp.path().join(STATE_FILE);
        let old_bytes = br#"{"threadIds":{"peer":"original"},"getUpdatesBuf":"cursor"}"#;
        std::fs::write(&published, old_bytes).unwrap();
        let store = ConnectSessionStore::in_directory(temp.path(), "  ");
        let mut state = store.load().unwrap();
        assert_eq!(state.thread_ids["peer"], "original");
        assert_eq!(state.get_updates_buf, "cursor");
        state.get_updates_buf = "next-cursor".into();
        store.save(&state).unwrap();
        assert_eq!(store.load().unwrap(), state);
        assert_eq!(std::fs::read(&published).unwrap(), old_bytes);
        assert!(
            ConnectSessionStore::in_directory(temp.path(), "default")
                .load()
                .unwrap()
                .thread_ids
                .is_empty()
        );
    }

    #[test]
    fn malformed_published_empty_account_state_is_not_silently_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let published = temp.path().join(STATE_FILE);
        std::fs::write(&published, b"not-json").unwrap();
        let store = ConnectSessionStore::in_directory(temp.path(), "");
        assert!(store.load().is_err());
        assert!(!store.path.exists());
        assert_eq!(std::fs::read(&published).unwrap(), b"not-json");
    }

    #[test]
    fn real_account_paths_are_case_sensitive_and_published_threads_remain_readable() {
        let temp = tempfile::tempdir().unwrap();
        let account = "Bot.1@wx.example";
        let published = temp
            .path()
            .join(format!("weixin-connect-state-{account}.json"));
        std::fs::write(&published, br#"{"threadIds":{"peer":"original"}}"#).unwrap();
        let store = ConnectSessionStore::in_directory(temp.path(), account);
        assert_eq!(store.load().unwrap().thread_ids["peer"], "original");
        let lower = ConnectSessionStore::in_directory(temp.path(), "bot.1@wx.example");
        assert_ne!(
            store.path.to_string_lossy().to_lowercase(),
            lower.path.to_string_lossy().to_lowercase()
        );
        assert!(lower.load().unwrap().thread_ids.is_empty());
        let state = store.load().unwrap();
        store.save(&state).unwrap();
        assert_eq!(store.load().unwrap().thread_ids["peer"], "original");
        assert!(published.exists());
        assert_ne!(
            ConnectSessionStore::in_directory(temp.path(), "a/b").path,
            ConnectSessionStore::in_directory(temp.path(), "a_b").path
        );
    }

    #[test]
    fn state_store_round_trips_and_limits_dedup_history() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConnectSessionStore::new(temp.path().join("state.json"));
        let mut state = ConnectState {
            get_updates_buf: "cursor".to_string(),
            ..ConnectState::default()
        };
        state
            .thread_ids
            .insert("peer".to_string(), "thread".to_string());
        for id in 1..=600 {
            state.mark_processed(format!("message-{id}"));
        }

        store.save(&state).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.get_updates_buf, "cursor");
        assert_eq!(
            loaded.thread_ids.get("peer").map(String::as_str),
            Some("thread")
        );
        assert!(!loaded.is_processed("message-1"));
        assert!(loaded.is_processed("message-600"));
    }

    #[test]
    fn persisted_inflight_message_requires_manual_retry_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConnectSessionStore::new(temp.path().join("queue.json"));
        let message: WeixinMessage = serde_json::from_value(serde_json::json!({
            "message_type": 1, "message_state": 2, "from_user_id": "peer",
            "message_id": 42, "context_token": "reply-token",
            "item_list": [{ "type": 1, "text_item": { "text": "hello" } }]
        }))
        .unwrap();
        store
            .update(|state| {
                assert!(state.enqueue(message, 123));
                state.get_updates_buf = "next-cursor".to_string();
                state.messages[0].state = "processing".to_string();
            })
            .unwrap();
        store.update(ConnectState::recover_interrupted).unwrap();
        let mut state = store.load().unwrap();
        assert_eq!(state.get_updates_buf, "next-cursor");
        assert_eq!(state.messages[0].state, "failed");
        assert_eq!(state.pending_count(), 0);
        assert!(!state.enqueue(state.messages[0].message.clone(), 124));
    }

    #[test]
    fn old_queued_message_json_defaults_the_reply_outbox_to_none() {
        let message: QueuedMessage = serde_json::from_value(serde_json::json!({
            "key": "peer|42|0|0|",
            "message": {
                "message_type": 1,
                "message_state": 2,
                "from_user_id": "peer",
                "message_id": 42,
                "context_token": "reply-token",
                "item_list": [{ "type": 1, "text_item": { "text": "hello" } }]
            },
            "state": "pending"
        }))
        .unwrap();

        assert_eq!(message.reply_outbox, None);
    }

    #[test]
    fn missing_context_token_is_persisted_as_failed_and_needs_a_fresh_token_to_retry() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConnectSessionStore::new(temp.path().join("queue.json"));
        let message: WeixinMessage = serde_json::from_value(serde_json::json!({
            "message_type": 1,
            "message_state": 2,
            "from_user_id": "peer",
            "message_id": 43,
            "item_list": [{ "type": 1, "text_item": { "text": "hello" } }]
        }))
        .unwrap();

        let key = message.dedup_key();
        store
            .update(|state| {
                assert!(state.enqueue(message, 456));
                assert!(!state.retry_failed_message(&key));
            })
            .unwrap();

        let mut state = store.load().unwrap();
        assert_eq!(state.failed_count(), 1);
        assert_eq!(state.messages[0].state, "failed");
        assert!(state.messages[0].error.contains("context_token"));
        assert_eq!(state.messages[0].key, key);
        assert!(state.set_latest_context_token("peer", "fresh-reply-token"));
        assert!(state.retry_failed_message(&key));
        assert_eq!(state.messages[0].state, "pending");
    }

    #[test]
    fn a_fresh_peer_update_requeues_only_messages_waiting_for_a_context_token() {
        let missing_token: WeixinMessage = serde_json::from_value(serde_json::json!({
            "message_type": 1,
            "message_state": 2,
            "from_user_id": "peer",
            "message_id": 45,
            "item_list": [{ "type": 1, "text_item": { "text": "first" } }]
        }))
        .unwrap();
        let mut state = ConnectState::default();
        assert!(state.enqueue(missing_token, 999));
        assert!(state.set_latest_context_token("peer", "fresh-token"));

        assert_eq!(state.retry_messages_waiting_for_context_token("peer"), 1);
        assert_eq!(state.messages[0].state, "pending");
        assert_eq!(state.messages[0].message.context_token, "fresh-token");
        assert_eq!(state.retry_messages_waiting_for_context_token("peer"), 0);
    }

    #[test]
    fn reply_outbox_progress_and_latest_token_survive_persistence() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConnectSessionStore::new(temp.path().join("queue.json"));
        let message: WeixinMessage = serde_json::from_value(serde_json::json!({
            "message_type": 1,
            "message_state": 2,
            "from_user_id": "peer",
            "message_id": 44,
            "context_token": "old-token",
            "item_list": [{ "type": 1, "text_item": { "text": "hello" } }]
        }))
        .unwrap();
        let key = message.dedup_key();
        let mut state = ConnectState::default();
        assert!(state.enqueue(message, 789));
        assert!(state.save_reply_outbox(
            &key,
            ReplyOutbox {
                thread_id: "thread-1".to_string(),
                text_chunks: vec!["first".to_string(), "second".to_string()],
                attachments: vec!["report.txt".to_string()],
                ..ReplyOutbox::default()
            }
        ));
        assert!(!state.save_reply_outbox(&key, ReplyOutbox::default()));
        assert!(state.set_latest_context_token("peer", "new-token"));

        assert!(state.mark_reply_text_chunk_delivered(&key, 0));
        assert!(!state.mark_reply_text_chunk_delivered(&key, 0));
        store.save(&state).unwrap();
        let mut state = store.load().unwrap();

        let (outbox, token) = state.reply_outbox_for_delivery(&key).unwrap();
        assert_eq!(token, "new-token");
        assert_eq!(outbox.thread_id, "thread-1");
        assert_eq!(outbox.delivered_text_chunks, 1);
        assert_eq!(outbox.next_text_chunk(), Some((1, "second")));
        assert_eq!(outbox.next_attachment(), None);

        assert!(state.mark_reply_text_chunk_delivered(&key, 1));
        let (outbox, _) = state.reply_outbox_for_delivery(&key).unwrap();
        assert_eq!(outbox.next_attachment(), Some((0, "report.txt")));
        assert!(state.mark_reply_attachment_delivered(&key, 0));
        let (outbox, _) = state.reply_outbox_for_delivery(&key).unwrap();
        assert!(outbox.is_delivered());
    }

    #[test]
    fn failed_message_blocks_later_messages_from_same_peer() {
        let mut state = ConnectState::default();
        for (peer, id) in [("alice", 1), ("alice", 2), ("bob", 3)] {
            let message = serde_json::from_value(serde_json::json!({
                "message_type": 1, "message_state": 2, "from_user_id": peer,
                "message_id": id, "context_token": "token",
                "item_list": [{ "type": 1, "text_item": { "text": "hello" } }]
            }))
            .unwrap();
            assert!(state.enqueue(message, id as u64));
        }
        state.messages[0].state = "failed".to_string();
        let (_, first, _, _) = state.claim_next(&BTreeSet::new()).unwrap();
        assert_eq!(first.from_user_id, "bob");
        state.messages[0].state = "pending".to_string();
        let (_, next, _, _) = state.claim_next(&BTreeSet::new()).unwrap();
        assert_eq!(next.message_id, 1);
    }

    #[test]
    fn expired_pending_messages_are_not_replayed_after_a_cursor_reset() {
        let message: WeixinMessage = serde_json::from_value(serde_json::json!({
            "message_type": 1,
            "message_state": 2,
            "from_user_id": "peer",
            "message_id": 46,
            "create_time_ms": 100,
            "context_token": "token",
            "item_list": [{ "type": 1, "text_item": { "text": "old" } }]
        }))
        .unwrap();
        let mut state = ConnectState::default();
        assert!(state.enqueue(message, 100));
        let key = state.messages[0].key.clone();

        assert_eq!(state.expire_old_pending_messages(1_000, 500), 1);
        assert_eq!(state.pending_count(), 0);
        assert_eq!(state.messages[0].state, "completed");
        assert!(state.is_processed(&key));
        assert!(state.claim_next(&BTreeSet::new()).is_none());
    }
}
