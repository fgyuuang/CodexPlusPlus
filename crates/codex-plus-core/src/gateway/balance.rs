//! 网关供应商的账户余额：有公开余额接口的几家（DeepSeek、Kimi / Moonshot、OpenRouter、SiliconFlow）
//! 用该供应商的第一个 Key 查；识别为中转站（sub2api / new-api）的，和共享供应商卡片走同一条识别链路。
//! 其它供应商不列出。

use std::{collections::BTreeMap, time::Duration};

use serde::Serialize;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

use crate::gateway::store::GatewaySettings;
use crate::settings::RelayProfile;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBalance {
    pub provider_id: String,
    pub provider: String,
    pub base_url: String,
    pub amount: Option<f64>,
    pub currency: String,
    pub error: Option<String>,
    /// 没有金额时的说明（如 new-api 的 Key 不限额度、没配访问令牌），界面放在悬停提示里。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_amount: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_coverage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_registered_at: Option<u64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub workbuddy_model_rates: BTreeMap<String, f64>,
    /// 当前调用 Key 自创建以来已记录的积分消耗，不是所选时间段或账号池消耗。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_used: Option<f64>,
}

pub fn has_workbuddy_usage(source: &str) -> bool {
    matches!(source, "workbuddy" | "codex:managed-workbuddy") || workbuddy_realm(source).is_some()
}

pub fn workbuddy_realm(source: &str) -> Option<&str> {
    source
        .strip_prefix("workbuddy:")
        .filter(|realm| matches!(*realm, "cn" | "global"))
}

/// 余额接口：地址和解析方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalanceApi {
    DeepSeek,
    Moonshot { global: bool },
    OpenRouter,
    SiliconFlow { global: bool },
}

impl BalanceApi {
    pub fn url(&self) -> &'static str {
        match self {
            BalanceApi::DeepSeek => "https://api.deepseek.com/user/balance",
            BalanceApi::Moonshot { global: false } => "https://api.moonshot.cn/v1/users/me/balance",
            BalanceApi::Moonshot { global: true } => "https://api.moonshot.ai/v1/users/me/balance",
            BalanceApi::OpenRouter => "https://openrouter.ai/api/v1/credits",
            BalanceApi::SiliconFlow { global: false } => "https://api.siliconflow.cn/v1/user/info",
            BalanceApi::SiliconFlow { global: true } => "https://api.siliconflow.com/v1/user/info",
        }
    }

    /// 解析接口返回，得到 (金额, 币种)。
    pub fn parse(&self, value: &Value) -> Option<(f64, String)> {
        match self {
            BalanceApi::DeepSeek => {
                let rows = value.get("balance_infos")?.as_array()?;
                let row = rows
                    .iter()
                    .find(|row| row.get("currency").and_then(Value::as_str) == Some("CNY"))
                    .or_else(|| rows.first())?;
                Some((
                    number(row.get("total_balance")?)?,
                    row.get("currency")
                        .and_then(Value::as_str)
                        .unwrap_or("CNY")
                        .to_string(),
                ))
            }
            BalanceApi::Moonshot { global } => Some((
                number(value.pointer("/data/available_balance")?)?,
                if *global { "USD" } else { "CNY" }.to_string(),
            )),
            BalanceApi::OpenRouter => {
                let total = number(value.pointer("/data/total_credits")?)?;
                let used = value
                    .pointer("/data/total_usage")
                    .and_then(number)
                    .unwrap_or(0.0);
                Some((total - used, "USD".to_string()))
            }
            BalanceApi::SiliconFlow { global } => Some((
                number(value.pointer("/data/totalBalance")?)?,
                if *global { "USD" } else { "CNY" }.to_string(),
            )),
        }
    }
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

/// 按供应商地址判断有没有余额接口。
pub fn balance_api(relay: &RelayProfile) -> Option<BalanceApi> {
    let base = relay.base_url.trim().to_ascii_lowercase();
    if base.contains("deepseek.com") {
        Some(BalanceApi::DeepSeek)
    } else if base.contains("moonshot.cn") {
        Some(BalanceApi::Moonshot { global: false })
    } else if base.contains("moonshot.ai") {
        Some(BalanceApi::Moonshot { global: true })
    } else if base.contains("openrouter.ai") {
        Some(BalanceApi::OpenRouter)
    } else if base.contains("siliconflow.cn") {
        Some(BalanceApi::SiliconFlow { global: false })
    } else if base.contains("siliconflow.com") {
        Some(BalanceApi::SiliconFlow { global: true })
    } else {
        None
    }
}

/// 共享供应商卡片据此显示余额（#2793）：有余额接口的供应商（DeepSeek 等）不当中转站识别，
/// 余额和「用量」页、托盘面板同一个来源（[`fetch_balances`]），界面共用一份查询结果。
pub fn has_balance_api(relay: &RelayProfile) -> bool {
    balance_api(relay).is_some()
}

/// 查池里所有打开的供应商的余额：有余额接口的厂商和识别出的中转站（托盘、「用量」页、卡片共用）。
/// 并发请求，每个最多等 10 秒；不是中转站、也没有余额接口的不列出。
pub async fn fetch_balances(gateway: &GatewaySettings) -> Vec<ProviderBalance> {
    let tasks = gateway
        .providers
        .iter()
        .filter(|provider| provider.enabled)
        .map(|provider| {
            let relay = provider.relay.clone();
            let access = provider.newapi_access.clone();
            let source = provider.source.clone();
            tokio::spawn(async move {
                if has_workbuddy_usage(&source) {
                    return Some(workbuddy_balance(&relay, workbuddy_realm(&source)).await);
                }
                match balance_api(&relay) {
                    Some(api) => Some(vendor_balance(&relay, api).await),
                    None => {
                        let probe = crate::relay_billing::probe_relay_billing_with_access(
                            &relay,
                            access.as_ref(),
                        )
                        .await;
                        relay_balance(&relay, &probe)
                    }
                }
            })
        })
        .collect::<Vec<_>>();
    let mut balances = Vec::new();
    for task in tasks {
        if let Ok(Some(balance)) = task.await {
            balances.push(balance);
        }
    }
    balances
}

async fn vendor_balance(relay: &RelayProfile, api: BalanceApi) -> ProviderBalance {
    let result = fetch_one(relay, api).await;
    ProviderBalance {
        provider_id: relay.id.clone(),
        provider: relay.name.clone(),
        base_url: relay.base_url.clone(),
        amount: result.as_ref().ok().map(|(amount, _)| *amount),
        currency: result
            .as_ref()
            .map(|(_, currency)| currency.clone())
            .unwrap_or_default(),
        error: result.err(),
        note: None,
        scope: None,
        pool_amount: None,
        pool_coverage: None,
        pool_note: None,
        pool_registered_at: None,
        workbuddy_model_rates: BTreeMap::new(),
        key_used: None,
    }
}

/// 中转站的余额，取自识别结果（和共享供应商卡片同一来源）；没识别出中转站时不列出。
fn relay_balance(
    relay: &RelayProfile,
    probe: &crate::relay_billing::RelayBillingProbe,
) -> Option<ProviderBalance> {
    probe.platform.as_ref()?;
    let usage = probe.usage.as_ref()?;
    let amount = usage.remaining.filter(|_| usage.kind != "unlimited");
    // 访问令牌查账户余额失败时给出原因；Key 不限额度又没配令牌时只是查不到，不算失败。
    let error = amount
        .is_none()
        .then(|| {
            probe
                .newapi_access
                .as_ref()
                .and_then(|status| status.error.clone())
        })
        .flatten();
    let note = (amount.is_none() && error.is_none() && usage.kind == "unlimited")
        .then(|| "Key 不限额度，在供应商详情里配置访问令牌后可显示账户余额".to_string());
    Some(ProviderBalance {
        provider_id: relay.id.clone(),
        provider: relay.name.clone(),
        base_url: relay.base_url.clone(),
        amount,
        currency: usage.unit.clone(),
        error,
        note,
        scope: None,
        pool_amount: None,
        pool_coverage: None,
        pool_note: None,
        pool_registered_at: None,
        workbuddy_model_rates: BTreeMap::new(),
        key_used: None,
    })
}

fn workbuddy_usage(value: &Value) -> Result<Option<f64>, String> {
    if value.get("object").and_then(Value::as_str) != Some("key.usage") {
        return Err("额度响应格式不正确".into());
    }
    let credits = value
        .get("credits")
        .and_then(Value::as_object)
        .ok_or_else(|| "额度响应缺少积分字段".to_string())?;
    match (credits.get("limit"), credits.get("remaining")) {
        (Some(Value::Null), Some(Value::Null)) => Ok(None),
        (Some(limit), Some(remaining)) => {
            let limit = number(limit).filter(|n| n.is_finite() && *n >= 0.0);
            let remaining = number(remaining).filter(|n| n.is_finite() && *n >= 0.0);
            match (limit, remaining) {
                (Some(_), Some(n)) => Ok(Some(n)),
                _ => Err("额度响应中的积分数值无效".into()),
            }
        }
        _ => Err("额度响应缺少限额信息".into()),
    }
}

async fn workbuddy_balance(relay: &RelayProfile, realm: Option<&str>) -> ProviderBalance {
    let result = async {
        let base = if relay.upstream_base_url.trim().is_empty() {
            relay.base_url.trim()
        } else {
            relay.upstream_base_url.trim()
        };
        let mut url = reqwest::Url::parse(base).map_err(|_| "服务地址无效".to_string())?;
        if !matches!(url.scheme(), "http" | "https")
            || !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path().trim_end_matches('/'), "" | "/v1")
        {
            return Err("服务地址不是受支持的本机地址".to_string());
        }
        url.set_path("/v1/usage");
        url.set_query(Some("codexpp=1"));
        let key = relay
            .selected_api_key()
            .map(|item| item.api_key.as_str())
            .filter(|key| !key.trim().is_empty())
            .unwrap_or(&relay.api_key)
            .trim();
        if key.is_empty() {
            return Err("没有 Key".to_string());
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| "无法创建额度查询".to_string())?;
        let mut response = client
            .get(url.clone())
            .bearer_auth(key)
            .send()
            .await
            .map_err(|_| "查询额度失败：连不上".to_string())?;
        if !response.status().is_success() {
            return Err(format!("查询额度失败：HTTP {}", response.status().as_u16()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "读取额度响应失败".to_string())?
        {
            if body.len() + chunk.len() > 1024 * 1024 {
                return Err("额度响应过大".to_string());
            }
            body.extend_from_slice(&chunk);
        }
        let value: Value =
            serde_json::from_slice(&body).map_err(|_| "额度响应不是 JSON".to_string())?;
        if let Some(realm) = realm
            && value.pointer("/key/realm").and_then(Value::as_str) != Some(realm)
        {
            return Err("调用 Key 的版本与此 WorkBuddy 供应商不匹配".into());
        }
        let amount = workbuddy_usage(&value)?;
        let used = value
            .pointer("/credits/used")
            .and_then(number)
            .filter(|n| n.is_finite() && *n >= 0.0);
        let pool_id = match value.pointer("/key/upstream_id") {
            Some(Value::Null) => Some(0),
            Some(value) => value.as_u64(),
            None => None,
        };
        let pool = pool_id.and_then(|id| account_pool_summary(&value, id));
        let rates: BTreeMap<String, f64> = value
            .get("model_credit_rates")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(id, value)| {
                let (realm, name) = id.split_once(':')?;
                if !matches!(realm, "cn" | "global") || name.is_empty() || name.len() > 200 {
                    return None;
                }
                let rate = value
                    .as_f64()
                    .filter(|rate| rate.is_finite() && *rate >= 0.0)?;
                Some((id.clone(), rate))
            })
            .collect();
        Ok::<_, String>((amount, pool, used, rates))
    }
    .await;
    let snapshot = result
        .as_ref()
        .ok()
        .and_then(|(_, pool, _, _)| pool.as_ref());
    ProviderBalance {
        provider_id: relay.id.clone(),
        provider: relay.name.clone(),
        base_url: relay.base_url.clone(),
        amount: result.as_ref().ok().and_then(|(amount, _, _, _)| *amount),
        currency: "积分".into(),
        note: result.as_ref().ok().map(|(amount, _, _, _)| {
            if amount.is_none() {
                "当前调用 Key 不限额度，不代表账号池余额".into()
            } else {
                "当前选中 Key 的限额，不代表账号池余额".into()
            }
        }),
        key_used: result.as_ref().ok().and_then(|(_, _, used, _)| *used),
        scope: Some("key".into()),
        pool_amount: snapshot.map(|(amount, _, _)| *amount),
        pool_coverage: snapshot.map(|(_, coverage, _)| coverage.clone()),
        pool_note: snapshot
            .is_none()
            .then(|| "WorkBuddy Manager 未返回此 Key 所属分组的余额".into()),
        pool_registered_at: snapshot.and_then(|(_, _, time)| *time),
        workbuddy_model_rates: result
            .as_ref()
            .ok()
            .map(|(_, _, _, rates)| rates.clone())
            .unwrap_or_default(),
        error: result.err(),
    }
}

fn account_pool_summary(value: &Value, expected_id: u64) -> Option<(f64, String, Option<u64>)> {
    let pool = value.get("account_pool")?;
    let id = pool.get("upstream_id")?;
    if !(expected_id == 0 && id.is_null()) && id.as_u64() != Some(expected_id) {
        return None;
    }
    let amount = pool
        .get("remaining")?
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)?;
    let observed = pool.get("observed")?.as_u64()?;
    let total = pool.get("total")?.as_u64()?;
    if observed == 0 || observed > total {
        return None;
    }
    let time = pool.get("oldest_registered_at").and_then(Value::as_u64);
    Some((amount, format!("{observed}/{total}"), time))
}

async fn fetch_one(relay: &RelayProfile, api: BalanceApi) -> Result<(f64, String), String> {
    fetch_one_at(relay, api, api.url()).await
}

/// 同 [`fetch_one`]，地址由调用方给（测试里换成本机的模拟上游）。
async fn fetch_one_at(
    relay: &RelayProfile,
    api: BalanceApi,
    url: &str,
) -> Result<(f64, String), String> {
    let key = relay.api_key.trim();
    if key.is_empty() {
        return Err("没有 Key".to_string());
    }
    // 余额查询属于这个供应商：按它自己的代理设置发（#2775）。
    let client = crate::http_client::proxied_client_for_relay("CodexPlusPlus", relay)
        .map_err(|error| format!("{error:#}"))?;
    let response = client
        .get(url)
        .bearer_auth(key)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| "查询余额失败：连不上".to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("查询余额失败：HTTP {}", status.as_u16()));
    }
    let value: Value = response
        .json()
        .await
        .map_err(|_| "余额响应不是 JSON".to_string())?;
    api.parse(&value)
        .ok_or_else(|| "余额响应里没有金额".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn workbuddy_usage_preserves_key_quota_semantics() {
        assert_eq!(
            workbuddy_usage(&json!({"object":"key.usage","credits":{"limit":100,"remaining":27}})),
            Ok(Some(27.0))
        );
        assert_eq!(
            workbuddy_usage(
                &json!({"object":"key.usage","credits":{"limit":null,"remaining":null}})
            ),
            Ok(None)
        );
        assert!(
            workbuddy_usage(&json!({"object":"key.usage","credits":{"limit":null,"remaining":0}}))
                .is_err()
        );
        assert!(workbuddy_usage(&json!({"credits":{"limit":100,"remaining":0}})).is_err());
    }

    #[test]
    fn account_pool_summary_requires_matching_group_and_reports_coverage() {
        let snapshot = json!({"account_pool":{"upstream_id":7,"remaining":10,"observed":2,"total":3,"oldest_registered_at":99}});
        assert_eq!(
            account_pool_summary(&snapshot, 7),
            Some((10.0, "2/3".into(), Some(99)))
        );
        assert_eq!(account_pool_summary(&snapshot, 8), None);
        assert_eq!(
            account_pool_summary(
                &json!({"account_pool":{"upstream_id":7,"remaining":null,"observed":0,"total":2}}),
                7
            ),
            None
        );
        assert_eq!(
            account_pool_summary(
                &json!({"account_pool":{"upstream_id":null,"remaining":0,"observed":1,"total":1}}),
                0
            ),
            Some((0.0, "1/1".into(), None))
        );
    }

    #[tokio::test]
    async fn workbuddy_usage_queries_selected_key_on_loopback_only() {
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/usage"))
            .and(query_param("codexpp", "1"))
            .and(header("authorization", "Bearer selected-test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "object":"key.usage", "credits":{"limit":100,"remaining":42,"used":58},
                "key":{"upstream_id":1},
                "account_pool":{"upstream_id":1,"remaining":3060.5,"observed":2,"total":2,"oldest_registered_at":99},
                "model_credit_rates":{"cn:free":0,"global:paid":1.33,"cn:invalid":-1,"bare":1}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let mut relay = relay(&server.uri());
        relay.api_key = "old-key".into();
        relay.api_keys = vec![crate::settings::RelayApiKey {
            id: "selected".into(),
            name: "Selected".into(),
            api_key: "selected-test-key".into(),
        }];
        relay.active_api_key_id = "selected".into();
        let balance = workbuddy_balance(&relay, None).await;
        assert_eq!(balance.amount, Some(42.0));
        assert_eq!(balance.key_used, Some(58.0));
        assert_eq!(balance.scope.as_deref(), Some("key"));
        assert_eq!(balance.pool_amount, Some(3060.5));
        assert_eq!(balance.pool_registered_at, Some(99));
        assert_eq!(balance.workbuddy_model_rates.get("cn:free"), Some(&0.0));
        assert_eq!(
            balance.workbuddy_model_rates.get("global:paid"),
            Some(&1.33)
        );
        assert_eq!(balance.workbuddy_model_rates.len(), 2);
        relay.base_url = "https://example.com/v1".into();
        assert!(workbuddy_balance(&relay, None).await.error.is_some());
    }

    fn relay(base_url: &str) -> RelayProfile {
        RelayProfile {
            base_url: base_url.to_string(),
            ..RelayProfile::default()
        }
    }

    #[test]
    fn recognizes_vendors_by_address() {
        assert_eq!(
            balance_api(&relay("https://api.deepseek.com/")),
            Some(BalanceApi::DeepSeek)
        );
        assert_eq!(
            balance_api(&relay("https://api.moonshot.cn/v1")),
            Some(BalanceApi::Moonshot { global: false })
        );
        assert_eq!(
            balance_api(&relay("https://openrouter.ai/api/v1")),
            Some(BalanceApi::OpenRouter)
        );
        assert_eq!(balance_api(&relay("https://relay.example/v1")), None);
    }

    #[test]
    fn parses_each_vendor_response() {
        let deepseek = json!({ "is_available": true, "balance_infos": [
            { "currency": "USD", "total_balance": "1.00" },
            { "currency": "CNY", "total_balance": "86.40" }
        ]});
        assert_eq!(
            BalanceApi::DeepSeek.parse(&deepseek),
            Some((86.4, "CNY".to_string()))
        );
        let moonshot = json!({ "data": { "available_balance": 142.1 } });
        assert_eq!(
            BalanceApi::Moonshot { global: false }.parse(&moonshot),
            Some((142.1, "CNY".to_string()))
        );
        let openrouter = json!({ "data": { "total_credits": 30.0, "total_usage": 6.25 } });
        assert_eq!(
            BalanceApi::OpenRouter.parse(&openrouter),
            Some((23.75, "USD".to_string()))
        );
        let silicon = json!({ "data": { "totalBalance": "12.5" } });
        assert_eq!(
            BalanceApi::SiliconFlow { global: false }.parse(&silicon),
            Some((12.5, "CNY".to_string()))
        );
        assert_eq!(BalanceApi::OpenRouter.parse(&json!({})), None);
    }

    #[test]
    fn pool_cards_show_balance_only_for_vendors_with_balance_api() {
        // 共享供应商卡片的余额来源（#2793）：DeepSeek 这类走余额接口，不当中转站识别；中转站不走。
        assert!(has_balance_api(&relay("https://api.deepseek.com")));
        assert!(has_balance_api(&relay("https://api.siliconflow.cn/v1")));
        assert!(!has_balance_api(&relay("https://jojocode.com/v1")));
    }

    fn relay_probe(kind: &str, remaining: Option<f64>) -> crate::relay_billing::RelayBillingProbe {
        crate::relay_billing::RelayBillingProbe {
            platform: Some("newapi".to_string()),
            rate: None,
            usage: Some(crate::relay_billing::RelayUsageInfo {
                endpoint: String::new(),
                kind: kind.to_string(),
                is_valid: true,
                plan_name: None,
                remaining,
                quota_limit: None,
                quota_used: None,
                unit: "USD".to_string(),
                rate_limits: Vec::new(),
            }),
            detail: String::new(),
            outcome: crate::relay_billing::RelayBillingOutcome::Recognized,
            newapi_access: None,
        }
    }

    #[test]
    fn relay_balances_come_from_the_billing_probe() {
        // 托盘「余额」和卡片同一来源：中转站也要列出，不能说「没有可以查询余额的供应商」。
        let provider = RelayProfile {
            id: "jojo".to_string(),
            name: "jojo".to_string(),
            ..relay("https://relay.example/v1")
        };
        let wallet = relay_balance(&provider, &relay_probe("wallet", Some(12.5))).unwrap();
        assert_eq!(wallet.amount, Some(12.5));
        assert_eq!(wallet.currency, "USD");
        assert_eq!(wallet.error, None);

        // Key 不限额度、没配访问令牌：列出但没有金额，提示去配令牌。
        let unlimited = relay_balance(&provider, &relay_probe("unlimited", None)).unwrap();
        assert_eq!(unlimited.amount, None);
        assert_eq!(unlimited.error, None);
        assert!(unlimited.note.unwrap().contains("访问令牌"));

        // 配了令牌但查询失败：给出原因。
        let mut failed = relay_probe("unlimited", None);
        failed.newapi_access = Some(crate::relay_billing::NewApiAccessStatus {
            configured: true,
            error: Some("HTTP 401：令牌无效".to_string()),
        });
        let failed = relay_balance(&provider, &failed).unwrap();
        assert_eq!(failed.error.as_deref(), Some("HTTP 401：令牌无效"));
        assert_eq!(failed.note, None);

        // 没识别出中转站：不列出。
        let unrecognized = crate::relay_billing::RelayBillingProbe::unrecognized(
            crate::relay_billing::RelayBillingOutcome::NotRelay,
            String::new(),
        );
        assert!(relay_balance(&provider, &unrecognized).is_none());
    }

    #[tokio::test]
    async fn pool_balances_include_recognized_relays() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/usage"))
            .and(header("authorization", "Bearer sk-relay"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"mode":"unrestricted","isValid":true,"planName":"钱包余额","remaining":8,"unit":"USD","balance":8}"#,
            ))
            .mount(&server)
            .await;
        let provider = crate::gateway::store::GatewayProvider {
            enabled: true,
            source: String::new(),
            price: None,
            model_prices: Default::default(),
            model_windows: Default::default(),
            context_window: None,
            proxy: String::new(),
            key_rpm: None,
            key_concurrency: None,
            upstream_new: Vec::new(),
            upstream_gone: Vec::new(),
            models_checked_at_ms: 0,
            newapi_access: None,
            subscription_policy: None,
            upstream_meta: Default::default(),
            auto_compact: Default::default(),
            relay: RelayProfile {
                id: "relay-a".to_string(),
                name: "中转 A".to_string(),
                api_key: "sk-relay".to_string(),
                relay_mode: crate::settings::RelayMode::PureApi,
                ..relay(&server.uri())
            },
            extra: Default::default(),
        };
        let mut disabled = provider.clone();
        disabled.enabled = false;
        disabled.relay.id = "relay-off".to_string();
        let gateway = GatewaySettings {
            providers: vec![provider, disabled],
            ..GatewaySettings::default()
        };

        let balances = fetch_balances(&gateway).await;

        assert_eq!(balances.len(), 1, "{balances:?}");
        assert_eq!(balances[0].provider_id, "relay-a");
        assert_eq!(balances[0].amount, Some(8.0));
        assert_eq!(balances[0].currency, "USD");
    }

    #[tokio::test]
    async fn deepseek_balance_comes_from_its_balance_api_with_provider_key() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/balance"))
            .and(header("authorization", "Bearer sk-deepseek"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "is_available": true,
                "balance_infos": [{ "currency": "CNY", "total_balance": "14.60" }]
            })))
            .mount(&server)
            .await;
        let provider = RelayProfile {
            api_key: "sk-deepseek".to_string(),
            ..relay("https://api.deepseek.com")
        };
        let url = format!("{}/user/balance", server.uri());

        let result = fetch_one_at(&provider, BalanceApi::DeepSeek, &url).await;

        assert_eq!(result, Ok((14.6, "CNY".to_string())));

        // 上游拒绝时给出原因，不带 Key。
        let failing = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&failing)
            .await;
        let error = fetch_one_at(
            &provider,
            BalanceApi::DeepSeek,
            &format!("{}/user/balance", failing.uri()),
        )
        .await
        .unwrap_err();
        assert_eq!(error, "查询余额失败：HTTP 401");
    }
}
