//! 独立、显式选择的官方账号调度；固定账号 provider 不使用此模块。
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use crate::official_accounts::{
    OfficialAccountStore, OfficialAccountSummary, OfficialUsageSnapshot,
};
use crate::settings::BackendSettings;
use serde_json::Value;

pub const PROVIDER_ID: &str = "codex_plus_official_schedule";
static BLOCKED: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();

pub(crate) fn eligible_model_entry(entry: &Value) -> bool {
    entry.get("supported_in_api").and_then(Value::as_bool) != Some(false)
        && entry
            .get("visibility")
            .and_then(Value::as_str)
            .is_none_or(|visibility| visibility.eq_ignore_ascii_case("list"))
}

pub fn ordered_accounts(
    settings: &BackendSettings,
    accounts: Vec<OfficialAccountSummary>,
) -> Vec<OfficialAccountSummary> {
    let mut seen = HashSet::new();
    settings
        .official_experience
        .account_schedule_order
        .iter()
        .filter(|id| seen.insert(id.as_str()))
        .filter_map(|id| {
            accounts
                .iter()
                .find(|a| &a.id == id && a.enabled && a.status == "ready")
                .cloned()
        })
        .collect()
}

pub fn quota_exhausted(usage: Option<&OfficialUsageSnapshot>, now: i64) -> bool {
    usage.is_some_and(|usage| {
        [&usage.primary, &usage.secondary]
            .into_iter()
            .flatten()
            .any(|w| {
                w.used_percent.is_some_and(|p| p.is_finite() && p >= 100.0)
                    && w.resets_at.is_some_and(|reset| reset > now)
            })
    })
}

/// 只识别服务端结构化额度错误，普通 429、容量错误、文本关键词均不算。
pub fn quota_error_reset(body: &[u8], now: i64) -> Option<i64> {
    let text = std::str::from_utf8(body).ok()?;
    let values = if let Ok(value) = serde_json::from_str::<Value>(text) {
        vec![value]
    } else {
        text.lines()
            .filter_map(|line| {
                line.strip_prefix("data:")
                    .and_then(|data| serde_json::from_str::<Value>(data.trim()).ok())
            })
            .collect()
    };
    // 任何输出/工具项出现后，禁止重放原请求。
    if values.iter().any(|v| {
        v.get("type").and_then(Value::as_str).is_some_and(|t| {
            t.contains(".delta")
                || t.starts_with("response.output_item.")
                || t.starts_with("response.content_part.")
        })
    }) {
        return None;
    }
    values.iter().find_map(|value| {
        let error = value
            .get("error")
            .or_else(|| value.pointer("/response/error"))?;
        let exhausted = ["code", "type"]
            .iter()
            .any(|key| error.get(key).and_then(Value::as_str) == Some("usage_limit_reached"));
        exhausted.then(|| {
            error
                .get("resets_at")
                .or_else(|| error.get("reset_at"))
                .and_then(Value::as_i64)
                .filter(|reset| *reset > now)
                .unwrap_or(now + 300)
        })
    })
}

pub fn mark_exhausted(account_id: &str, until: i64) {
    BLOCKED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(account_id.to_string(), until);
}

pub fn select_account(
    settings: &BackendSettings,
    model: &str,
    attempted: &HashSet<String>,
) -> anyhow::Result<OfficialAccountSummary> {
    anyhow::ensure!(
        settings.official_experience.enabled
            && settings.official_experience.account_schedule_enabled,
        "官方账号顺序调度未启用；固定账号不会自动切换"
    );
    let now = chrono::Utc::now().timestamp();
    let blocked = {
        let mut map = BLOCKED
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        map.retain(|_, reset| *reset > now);
        map.clone()
    };
    ordered_accounts(settings, OfficialAccountStore::default().list()?).into_iter()
        .filter(|a| !attempted.contains(&a.id) && !blocked.contains_key(&a.id) && !quota_exhausted(a.usage.as_ref(), now))
        .find(|a| crate::official_model_catalog::cached_official_model_entries_for_account(&a.id)
            .is_some_and(|entries| entries.iter().any(|e| e.get("slug").and_then(Value::as_str)
                .is_some_and(|slug| slug.eq_ignore_ascii_case(model)) && eligible_model_entry(e))))
        .ok_or_else(|| anyhow::anyhow!("顺序调度没有可用账号：账号额度已用尽，或账号未就绪/缺少目标模型目录；不会回退到登录账号或供应商"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn schedule_rejects_hidden_and_unsupported_account_models() {
        assert!(eligible_model_entry(
            &json!({"slug":"gpt-6.1-sol","visibility":"list"})
        ));
        assert!(!eligible_model_entry(
            &json!({"slug":"gpt-6.1-sol","visibility":"hide"})
        ));
        assert!(!eligible_model_entry(
            &json!({"slug":"gpt-6.1-sol","supported_in_api":false})
        ));
    }
}
