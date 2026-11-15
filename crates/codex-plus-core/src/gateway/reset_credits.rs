//! ChatGPT 订阅账号的「额度重置卡」：认得出、看得见、按确认用掉一张。
//!
//! `wham/usage` 给的是 5 小时 / 每周窗口的用量百分比；重置卡是另一份数据
//! （`wham/rate-limit-reset-credits`）：账号手上有几张、什么时候到期、能不能用。
//! 消费走同一前缀的 `/consume`，只要一个幂等请求标识，服务端自己挑一张可用卡。
//!
//! 消费是不可撤销的扣减：调用方必须先拿到用户的明确确认，且同一次操作只能用同一个
//! 请求标识重试——换标识重发等于再扣一张。
//!
//! 只在内存缓存；重启后重新查。读不到就当作「不知道」，不显示成 0 张。

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::settings::RelayProfile;

const CHATGPT_RESET_CREDITS_URL: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
/// 消费地址就是查询地址加 `/consume`。
const CHATGPT_RESET_CONSUME_SUFFIX: &str = "/consume";
/// 只认这一类卡：其它重置类型（例如别的产品的额度）不属于 Codex 窗口。
const RESET_TYPE: &str = "codex_rate_limits";
const MAX_CREDITS: usize = 1000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// 一张重置卡。`usable` 是这里算出来的：上游给了 `status`，但过期与否要按当前时间判。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCredit {
    pub id: String,
    /// `available` / `redeemed` / `expired` / `redeeming` / `pending` / `unknown`。
    pub status: String,
    /// 发到手的时间（毫秒时间戳）。
    pub granted_at_ms: Option<u64>,
    /// 到期时间（毫秒时间戳）；没有到期时间表示不会过期。
    pub expires_at_ms: Option<u64>,
    /// 现在就能用：状态是 `available` 且没过期。
    pub usable: bool,
}

/// 一个账号手上的全部重置卡，外加列表接口给的汇总数。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCreditsView {
    /// 上游汇总的可用张数；列表接口没给就为空，别拿 `credits` 里数出来的顶替。
    pub available_count: Option<u64>,
    /// 当前套餐下能用的张数。
    pub applicable_available_count: Option<u64>,
    /// 累计获得过的张数。
    pub total_earned_count: Option<u64>,
    pub credits: Vec<ResetCredit>,
    /// 这次读到的时间（毫秒时间戳）。
    pub observed_at_ms: u64,
}

impl ResetCreditsView {
    /// 现在能用的张数：优先用逐张判定的结果，没有明细时才退回汇总数。
    pub fn usable_count(&self) -> Option<u64> {
        if self.credits.is_empty() {
            return self.applicable_available_count.or(self.available_count);
        }
        Some(self.credits.iter().filter(|credit| credit.usable).count() as u64)
    }
}

/// 消费的结果。`Unknown` 必须按「可能已扣、也可能没扣」处理：让用户保留同一请求标识，
/// 不要另建一次操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumeOutcome {
    /// 这次真的重置了。
    Reset,
    /// 同一请求标识之前已经重置过，这次没有重复扣。
    AlreadyRedeemed,
    /// 服务端返回了别的 code：不能断言成功，也不能断言失败。
    Unknown,
}

impl ConsumeOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reset => "reset",
            Self::AlreadyRedeemed => "alreadyRedeemed",
            Self::Unknown => "unknown",
        }
    }
}

static CACHE: LazyLock<Mutex<HashMap<String, ResetCreditsView>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, ResetCreditsView>> {
    CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 读缓存；没查过返回 `None`（界面上不显示成 0 张）。
pub fn cached(relay_id: &str) -> Option<ResetCreditsView> {
    lock().get(relay_id).cloned()
}

pub fn record(relay_id: &str, view: ResetCreditsView) {
    lock().insert(relay_id.to_string(), view);
}

pub fn forget(relay_id: &str) {
    lock().remove(relay_id);
}

/// 秒级时间戳或 ISO 时间串转毫秒；不合法返回 `None`。
fn epoch_ms(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    if let Some(seconds) = value.as_f64() {
        if seconds > 0.0 {
            return Some((seconds * 1000.0) as u64);
        }
        return None;
    }
    let text = value.as_str()?;
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.timestamp_millis().max(0) as u64)
}

fn count(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64)
}

/// 解析 `wham/rate-limit-reset-credits`。
///
/// 结构不认识就返回 `None`——宁可显示「查不到」，也不要把有卡显示成没卡。
pub fn parse_credits(value: &Value, now: u64) -> Option<ResetCreditsView> {
    let object = value.as_object()?;
    let raw = object.get("credits")?;
    // 没有 `credits` 数组说明接口换了形状；空数组是合法的「一张都没有」。
    let list = raw.as_array()?;
    if list.len() > MAX_CREDITS {
        return None;
    }
    let credits = list
        .iter()
        .filter_map(|item| {
            let item = item.as_object()?;
            if item.get("reset_type")?.as_str()? != RESET_TYPE {
                return None;
            }
            let id = item.get("id")?.as_str()?.trim().to_string();
            if id.is_empty() || id.len() > 256 {
                return None;
            }
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .filter(|status| {
                    matches!(
                        *status,
                        "available" | "redeemed" | "expired" | "redeeming" | "pending"
                    )
                })
                .unwrap_or("unknown")
                .to_string();
            let expires_at_ms = epoch_ms(item.get("expires_at"));
            let expiry_passed = expires_at_ms.is_some_and(|at| at <= now);
            Some(ResetCredit {
                id,
                usable: status == "available" && !expiry_passed,
                status,
                granted_at_ms: epoch_ms(item.get("granted_at")),
                expires_at_ms,
            })
        })
        .collect();
    Some(ResetCreditsView {
        available_count: count(object.get("available_count")),
        applicable_available_count: count(object.get("applicable_available_count")),
        total_earned_count: count(object.get("total_earned_count")),
        credits,
        observed_at_ms: now,
    })
}

/// 是不是能查重置卡的账号：只有 ChatGPT 订阅（官方登录传输）有。
pub fn supported(relay: &RelayProfile) -> bool {
    crate::official_login::member_uses_official_login_transport(relay)
}

fn client_for(relay: &RelayProfile) -> anyhow::Result<reqwest::Client> {
    crate::http_client::build_for_relay(reqwest::Client::builder().timeout(REQUEST_TIMEOUT), relay)
}

async fn fetch(relay: &RelayProfile) -> anyhow::Result<ResetCreditsView> {
    if !supported(relay) {
        anyhow::bail!("只有 ChatGPT 订阅账号能查重置卡");
    }
    let credential = crate::official_login::resolve_official_credential_for_quota(relay).await?;
    let mut request = client_for(relay)?
        .get(CHATGPT_RESET_CREDITS_URL)
        .bearer_auth(&credential.access_token)
        .header(reqwest::header::ACCEPT, "application/json");
    if !credential.account_id.is_empty() {
        request = request.header("chatgpt-account-id", credential.account_id.as_str());
    }
    let response = request.send().await?;
    let status = response.status();
    if status.as_u16() == 404 {
        anyhow::bail!("这个账号不支持重置卡");
    }
    if !status.is_success() {
        anyhow::bail!("查询重置卡失败：HTTP {}", status.as_u16());
    }
    let now = now_ms();
    parse_credits(&response.json::<Value>().await?, now)
        .ok_or_else(|| anyhow::anyhow!("重置卡响应格式不认识，保留上次读数"))
}

/// 查一次并写缓存。查不到时保留上次读数，由调用方决定怎么提示。
pub async fn refresh(relay: &RelayProfile) -> anyhow::Result<ResetCreditsView> {
    let view = fetch(relay).await?;
    record(&relay.id, view.clone());
    Ok(view)
}

/// 缓存超过 `max_age_ms` 才真的去查，用来挡界面的重复刷新。
pub async fn refresh_if_stale(
    relay: &RelayProfile,
    max_age_ms: u64,
) -> anyhow::Result<Option<ResetCreditsView>> {
    let now = now_ms();
    if let Some(view) = cached(&relay.id)
        && now.saturating_sub(view.observed_at_ms) < max_age_ms
    {
        return Ok(Some(view));
    }
    refresh(relay).await.map(Some)
}

/// 请求标识只要稳定、可重放就够了：非空、不太长、没有控制字符。
pub fn valid_request_id(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 128
        && trimmed == value
        && !trimmed.chars().any(|ch| ch.is_control())
}

/// 用掉一张重置卡。
///
/// `request_id` 由调用方生成并保存：网络中断时用同一个标识重试，
/// 服务端据此判断是否已经处理过（返回 `already_redeemed`），不会再扣一张。
pub async fn consume(relay: &RelayProfile, request_id: &str) -> anyhow::Result<ConsumeOutcome> {
    if !valid_request_id(request_id) {
        anyhow::bail!("重置请求标识无效");
    }
    if !supported(relay) {
        anyhow::bail!("只有 ChatGPT 订阅账号能使用重置卡");
    }
    let credential = crate::official_login::resolve_official_credential_for_quota(relay).await?;
    let url = format!("{CHATGPT_RESET_CREDITS_URL}{CHATGPT_RESET_CONSUME_SUFFIX}");
    let mut request = client_for(relay)?
        .post(url)
        .bearer_auth(&credential.access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({ "redeem_request_id": request_id }));
    if !credential.account_id.is_empty() {
        request = request.header("chatgpt-account-id", credential.account_id.as_str());
    }
    let response = request.send().await?;
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        anyhow::bail!("登录凭据失效，未使用重置卡");
    }
    if !status.is_success() {
        anyhow::bail!("使用重置卡失败：HTTP {}", status.as_u16());
    }
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let outcome = match body.get("code").and_then(Value::as_str) {
        Some("reset") => ConsumeOutcome::Reset,
        Some("already_redeemed") => ConsumeOutcome::AlreadyRedeemed,
        _ => ConsumeOutcome::Unknown,
    };
    // 结果不确定也清缓存：下次读到的才是服务端的真实状态。
    forget(&relay.id);
    Ok(outcome)
}

/// 生成一个可以重放的请求标识（UUID v4）。
pub fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn card(id: &str, status: &str, expires_at: Option<i64>) -> Value {
        json!({
            "id": id,
            "reset_type": "codex_rate_limits",
            "status": status,
            "granted_at": 1_700_000_000,
            "expires_at": expires_at,
        })
    }

    #[test]
    fn parses_available_and_expired_cards() {
        let now = 1_700_000_000_000;
        let value = json!({
            "credits": [
                card("a", "available", Some(1_700_000_500)),
                card("b", "available", Some(1_600_000_000)),
                card("c", "redeemed", Some(1_700_000_500)),
                { "id": "d", "reset_type": "other_product", "status": "available" },
                { "id": "e", "reset_type": "codex_rate_limits", "status": "available", "expires_at": 1_700_000_500 },
            ],
            "available_count": 3,
            "applicable_available_count": 2,
            "total_earned_count": 5,
        });
        let view = parse_credits(&value, now).unwrap();
        // 只留 codex_rate_limits（d 是别的产品，丢掉）；过期的 available 不算可用。
        assert_eq!(view.credits.len(), 4);
        assert_eq!(view.credits[0].id, "a");
        assert!(view.credits[0].usable);
        assert_eq!(view.credits[1].id, "b");
        assert!(view.credits[1].expires_at_ms.is_some());
        assert!(!view.credits[1].usable);
        assert_eq!(view.credits[2].id, "c");
        assert!(!view.credits[2].usable);
        assert_eq!(view.credits[3].id, "e");
        assert!(view.credits[3].usable);
        assert_eq!(view.available_count, Some(3));
        assert_eq!(view.applicable_available_count, Some(2));
        assert_eq!(view.total_earned_count, Some(5));
        assert_eq!(view.usable_count(), Some(2));
    }

    #[test]
    fn empty_list_is_zero_cards_not_unknown() {
        let view = parse_credits(&json!({ "credits": [] }), 1).unwrap();
        assert!(view.credits.is_empty());
        assert_eq!(view.available_count, None);
        assert_eq!(view.usable_count(), None);
    }

    #[test]
    fn unknown_shape_is_none() {
        assert!(parse_credits(&json!({ "items": [] }), 1).is_none());
        assert!(parse_credits(&json!([]), 1).is_none());
    }

    #[test]
    fn request_id_rules() {
        assert!(valid_request_id("2b1f3a4c-0000-4000-8000-000000000000"));
        assert!(!valid_request_id(""));
        assert!(!valid_request_id("  x  "));
        assert!(!valid_request_id("bad\nid"));
        assert!(!valid_request_id(&"x".repeat(200)));
        assert!(valid_request_id(&new_request_id()));
    }
}
