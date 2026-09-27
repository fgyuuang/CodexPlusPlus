use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use anyhow::Context;
use serde::{Deserialize, Serialize};

const STATE_FILE: &str = "weixin-connect-state.json";
const MAX_PROCESSED_IDS: usize = 512;

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
    ) -> Option<(String, WeixinMessage, Option<String>)> {
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
        Some((item.key.clone(), message, prior_thread))
    }

    pub fn enqueue(&mut self, message: WeixinMessage, now_ms: u64) -> bool {
        let key = message.dedup_key();
        if self.is_processed(&key) || self.messages.iter().any(|queued| queued.key == key) {
            return false;
        }
        self.messages.push_back(QueuedMessage {
            key,
            message,
            state: "pending".to_string(),
            error: String::new(),
            received_at_ms: now_ms,
        });
        true
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
}

impl ConnectSessionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_for_account(account_id: &str) -> Self {
        let account = sanitize_account_id(account_id);
        let file_name = if account == "default" {
            STATE_FILE.to_string()
        } else {
            format!("weixin-connect-state-{account}.json")
        };
        Self::new(crate::paths::default_app_state_dir().join(file_name))
    }

    pub fn load(&self) -> anyhow::Result<ConnectState> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("无法解析微信连接状态 {}", self.path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
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
        let (_, first, _) = state.claim_next(&BTreeSet::new()).unwrap();
        assert_eq!(first.from_user_id, "bob");
        state.messages[0].state = "pending".to_string();
        let (_, next, _) = state.claim_next(&BTreeSet::new()).unwrap();
        assert_eq!(next.message_id, 1);
    }
}
