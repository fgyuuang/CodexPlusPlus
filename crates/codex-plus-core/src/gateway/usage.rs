//! 用量统计：每个请求结束时按「日期 × Agent × 供应商 × 模型」累加，存在 `<状态目录>/gateway-usage.json`。
//!
//! 请求日志会被裁剪到 1MB 左右，撑不起 30 天 / 全部的统计，所以单独存按天汇总的数字（不含请求内容）。
//! 统计覆盖经过 Codex++ 的全部流量：网关里的 Agent 按自己的名字，Codex 自己的请求记为 `codex`。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use anyhow::Context;
use chrono::Timelike;
use serde::{Deserialize, Serialize};

use crate::gateway::balance::has_workbuddy_usage;
use crate::gateway::store::{GatewaySettings, ProviderPrice};
use crate::request_log::{FINAL_RESPONSE_EVIDENCE_V1, RequestLogEntry};

const USAGE_FILE: &str = "gateway-usage.json";

/// 一组请求的累计数字。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCounts {
    #[serde(default)]
    pub calls: u64,
    #[serde(default)]
    pub errors: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    /// 输入里命中缓存的部分（已包含在 `input_tokens` 里）。
    #[serde(default)]
    pub cached_tokens: u64,
    /// 推理 token（已包含在 `output_tokens` 里）。
    #[serde(default)]
    pub reasoning_tokens: u64,
    /// 记了耗时的请求的总耗时（毫秒）；旧数据没有，平均耗时只按 `timed_calls` 算。
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub timed_calls: u64,
    /// 其中成功的生图请求（生图一般不返回 token，只能按次数看）。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub images: u64,
    /// 只加总 WorkBuddy 实际报告的积分；旧数据不填 0。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workbuddy_credits: Option<f64>,
    /// Number of calls with a usage receipt. Older rows cannot establish coverage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_metered_calls: Option<u64>,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl UsageCounts {
    fn add(&mut self, other: &UsageCounts) {
        self.calls += other.calls;
        self.errors += other.errors;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cached_tokens += other.cached_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
        self.duration_ms += other.duration_ms;
        self.timed_calls += other.timed_calls;
        self.images += other.images;
        if let Some(credits) = other.workbuddy_credits {
            self.workbuddy_credits = Some(self.workbuddy_credits.unwrap_or(0.0) + credits);
        }
        if let Some(metered) = other.token_metered_calls {
            self.token_metered_calls = Some(self.token_metered_calls.unwrap_or(0) + metered);
        }
    }
}

/// 某一天里某个 Agent 经某个供应商用某个模型的累计。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRow {
    pub agent: String,
    pub provider_id: String,
    pub provider: String,
    pub model: String,
    #[serde(flatten)]
    pub counts: UsageCounts,
    /// Portion whose account realm was recognizable from the recorded upstream model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scoped_workbuddy_credits: Option<f64>,
    /// 不认识的字段（更新版本加的统计项）原样保留：回滚后这一版继续累加时不把它们丢掉。
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageFile {
    #[serde(default)]
    version: u32,
    /// `YYYY-MM-DD`（本地日期）→ 当天各组合的累计。
    #[serde(default)]
    days: BTreeMap<String, Vec<UsageRow>>,
    /// 最近的本地日期及其小时汇总，不保存单条请求。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    hourly: BTreeMap<String, BTreeMap<u8, UsageCounts>>,
    /// 不认识的顶层字段原样保留。
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct UsageStore {
    pub path: PathBuf,
}

impl Default for UsageStore {
    fn default() -> Self {
        Self {
            path: crate::paths::default_app_state_dir().join(USAGE_FILE),
        }
    }
}

impl UsageStore {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn load(&self) -> anyhow::Result<UsageFile> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) if text.trim().is_empty() => Ok(UsageFile::default()),
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("无法解析 {}", self.path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(UsageFile::default()),
            Err(error) => Err(error).with_context(|| format!("无法读取 {}", self.path.display())),
        }
    }

    fn save(&self, file: &UsageFile) -> anyhow::Result<()> {
        let mut file = file.clone();
        file.version = 1;
        let text = serde_json::to_string(&file)?;
        crate::settings::atomic_write(&self.path, text.as_bytes())
            .with_context(|| format!("无法写入 {}", self.path.display()))
    }

    /// 记一个请求。`day` 是本地日期 `YYYY-MM-DD`。
    pub fn record(&self, day: &str, row: UsageRow) -> anyhow::Result<()> {
        self.record_at(day, None, row)
    }

    fn record_at(&self, day: &str, hour: Option<u8>, row: UsageRow) -> anyhow::Result<()> {
        let _guard = write_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load()?;
        if let Some(hour) = hour {
            file.hourly
                .entry(day.to_string())
                .or_default()
                .entry(hour)
                .or_default()
                .add(&row.counts);
            while file.hourly.len() > 2 {
                file.hourly.pop_first();
            }
        }
        let rows = file.days.entry(day.to_string()).or_default();
        match rows.iter_mut().find(|known| {
            known.agent == row.agent
                && known.provider_id == row.provider_id
                && known.model == row.model
        }) {
            Some(known) => {
                known.counts.add(&row.counts);
                if let Some(credits) = row.scoped_workbuddy_credits {
                    known.scoped_workbuddy_credits =
                        Some(known.scoped_workbuddy_credits.unwrap_or(0.0) + credits);
                }
                known.provider = row.provider;
            }
            None => rows.push(row),
        }
        self.save(&file)
    }

    /// 清空全部用量统计。
    pub fn clear(&self) -> anyhow::Result<()> {
        let _guard = write_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.save(&UsageFile::default())
    }

    /// 汇总 `days` 天（含今天；None 表示全部）的用量。`today` 是本地日期。
    pub fn summary(
        &self,
        today: &str,
        days: Option<u32>,
        gateway: &GatewaySettings,
    ) -> anyhow::Result<UsageSummary> {
        let file = self.load()?;
        let first = match days {
            Some(days) => shift_day(today, -(i64::from(days.max(1)) - 1)),
            None => file
                .days
                .keys()
                .next()
                .cloned()
                .unwrap_or_else(|| today.to_string()),
        };
        let mut summary = UsageSummary::default();
        if days == Some(1) {
            summary.hours = Some(
                file.hourly
                    .get(today)
                    .into_iter()
                    .flatten()
                    .map(|(&hour, &counts)| UsageHour { hour, counts })
                    .collect(),
            );
        }
        let mut day = first.clone();
        // 逐天补齐，没有请求的日子也画一根空柱子。
        let mut guard = 0;
        while day.as_str() <= today && guard < 3700 {
            let rows = file.days.get(&day).cloned().unwrap_or_default();
            let mut counts = UsageCounts::default();
            for row in &rows {
                counts.add(&row.counts);
                summary.totals.add(&row.counts);
                let price = gateway
                    .provider(&row.provider_id)
                    .filter(|provider| {
                        row.counts.workbuddy_credits.is_none()
                            && !has_workbuddy_usage(&provider.source)
                    })
                    .and_then(|provider| provider.price_for(&row.model));
                add_group(
                    &mut summary.by_agent,
                    &row.agent,
                    &row.agent,
                    &row.counts,
                    price,
                );
                add_group(
                    &mut summary.by_provider,
                    &row.provider_id,
                    &row.provider,
                    &row.counts,
                    price,
                );
                add_group(
                    &mut summary.by_model,
                    &row.model,
                    &row.model,
                    &row.counts,
                    price,
                );
                if let Some(price) = price {
                    add_cost(&mut summary.costs, price, &row.counts);
                }
                summary.daily_routes.push(UsageRouteDay {
                    date: day.clone(),
                    agent: row.agent.clone(),
                    provider_id: row.provider_id.clone(),
                    provider: row.provider.clone(),
                    model: row.model.clone(),
                    counts: row.counts,
                    credit_meter_id: row
                        .scoped_workbuddy_credits
                        .and_then(|_| workbuddy_meter_id(&row.model)),
                    scoped_workbuddy_credits: row.scoped_workbuddy_credits,
                });
            }
            summary.days.push(UsageDay {
                date: day.clone(),
                counts,
            });
            day = shift_day(&day, 1);
            guard += 1;
        }
        if let (Some(first), Some(last)) =
            (summary.daily_routes.first(), summary.daily_routes.last())
        {
            summary.observed_date_range = Some(UsageDateRange {
                first: first.date.clone(),
                last: last.date.clone(),
            });
        }
        for groups in [
            &mut summary.by_agent,
            &mut summary.by_provider,
            &mut summary.by_model,
        ] {
            groups.sort_by(|left, right| {
                (right.counts.input_tokens + right.counts.output_tokens)
                    .cmp(&(left.counts.input_tokens + left.counts.output_tokens))
                    .then(right.counts.calls.cmp(&left.counts.calls))
            });
        }
        Ok(summary)
    }
}

fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(Mutex::default)
}

fn add_group(
    groups: &mut Vec<UsageGroup>,
    key: &str,
    label: &str,
    counts: &UsageCounts,
    price: Option<&ProviderPrice>,
) {
    let index = match groups.iter().position(|group| group.key == key) {
        Some(index) => index,
        None => {
            groups.push(UsageGroup {
                key: key.to_string(),
                label: label.to_string(),
                ..UsageGroup::default()
            });
            groups.len() - 1
        }
    };
    let group = &mut groups[index];
    group.counts.add(counts);
    if let Some(price) = price {
        add_cost(&mut group.costs, price, counts);
    }
}

/// 优先显示实际扣费；其余供应商按填入的价格估算。
pub fn entry_cost(entry: &RequestLogEntry, gateway: &GatewaySettings) -> Option<UsageCost> {
    let usage = entry.usage.as_ref()?;
    if let Some(amount) = usage.workbuddy_credits {
        return Some(UsageCost {
            currency: "积分".into(),
            amount,
        });
    }
    if usage.input_tokens == 0 && usage.output_tokens == 0 {
        return None;
    }
    if gateway
        .provider(&entry.relay_id)
        .is_some_and(|provider| has_workbuddy_usage(&provider.source))
    {
        return None;
    }
    entry_pricing(entry, gateway).estimate
}

fn workbuddy_meter_id(model: &str) -> Option<&'static str> {
    let (realm, name) = model.split_once(':')?;
    if name.is_empty() {
        return None;
    }
    match realm {
        "cn" => Some("workbuddy:cn"),
        "global" => Some("workbuddy:global"),
        _ => None,
    }
}

pub fn entry_pricing(entry: &RequestLogEntry, gateway: &GatewaySettings) -> RequestUsagePricing {
    let mut pricing = RequestUsagePricing::default();
    let usage = entry.usage.as_ref();
    if let Some(amount) = usage
        .and_then(|usage| usage.workbuddy_credits)
        .filter(|amount| amount.is_finite() && *amount >= 0.0)
    {
        pricing.actual_meters.push(ActualUsageMeter {
            meter_id: workbuddy_meter_id(&entry.upstream_model).unwrap_or("workbuddy:unclassified"),
            unit: "credits",
            amount,
            source: "upstream-terminal-report",
        });
    }

    let model = entry.upstream_model.trim();
    let provider = gateway.provider(&entry.relay_id);
    let price = provider
        .filter(|_| !model.is_empty())
        .and_then(|provider| provider.price_for(model));
    if let Some(price) = price.filter(|price| {
        !price.currency.trim().is_empty()
            && [price.input, price.cached_input, price.output]
                .iter()
                .all(|rate| rate.is_finite() && *rate >= 0.0)
    }) {
        pricing.rate = Some(RequestUsageRate {
            model: model.to_string(),
            currency: price.currency.trim().to_ascii_uppercase(),
            input_per_million: price.input,
            cached_input_per_million: price.cached_input,
            output_per_million: price.output,
            basis: "current-config",
        });
    }

    if entry.state.as_deref() == Some("running") || entry.is_image() || !entry.success {
        pricing.estimate_status = EstimateStatus::NotApplicable;
        return pricing;
    }
    let Some(usage) = usage else {
        return pricing;
    };
    let Some(rate) = &pricing.rate else {
        pricing.estimate_status = EstimateStatus::MissingRate;
        return pricing;
    };
    if provider
        .is_some_and(|provider| crate::official_login::is_official_login_profile(&provider.relay))
    {
        pricing.estimate_status = EstimateStatus::UnknownTier;
        return pricing;
    }
    let Some(cached) = usage
        .reported_cached_tokens(
            entry.final_response_evidence.as_deref() == Some(FINAL_RESPONSE_EVIDENCE_V1),
        )
        .or_else(|| {
            (usage.input_tokens == 0 || rate.input_per_million == rate.cached_input_per_million)
                .then_some(0)
        })
    else {
        pricing.estimate_status = EstimateStatus::UnknownCache;
        return pricing;
    };
    let amount = (usage.input_tokens - cached) as f64 / 1_000_000.0 * rate.input_per_million
        + cached as f64 / 1_000_000.0 * rate.cached_input_per_million
        + usage.output_tokens as f64 / 1_000_000.0 * rate.output_per_million;
    if amount.is_finite() {
        pricing.estimate = Some(UsageCost {
            currency: rate.currency.clone(),
            amount,
        });
        pricing.estimate_status = EstimateStatus::Available;
    } else {
        pricing.estimate_status = EstimateStatus::MissingRate;
    }
    pricing
}

/// 按每百万 token 的价格估算费用：未命中缓存的输入、命中缓存的输入、输出分别计价。
fn add_cost(costs: &mut Vec<UsageCost>, price: &ProviderPrice, counts: &UsageCounts) {
    let uncached = counts.input_tokens.saturating_sub(counts.cached_tokens) as f64;
    let amount = (uncached * price.input
        + counts.cached_tokens as f64 * price.cached_input
        + counts.output_tokens as f64 * price.output)
        / 1_000_000.0;
    let currency = if price.currency.trim().is_empty() {
        "USD".to_string()
    } else {
        price.currency.trim().to_ascii_uppercase()
    };
    match costs.iter_mut().find(|cost| cost.currency == currency) {
        Some(cost) => cost.amount += amount,
        None => costs.push(UsageCost { currency, amount }),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDay {
    pub date: String,
    #[serde(flatten)]
    pub counts: UsageCounts,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageGroup {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub counts: UsageCounts,
    /// 这一组里填了价格的部分估算出的费用。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub costs: Vec<UsageCost>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCost {
    pub currency: String,
    pub amount: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestUsagePricing {
    pub actual_meters: Vec<ActualUsageMeter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<RequestUsageRate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimate: Option<UsageCost>,
    pub estimate_status: EstimateStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActualUsageMeter {
    pub meter_id: &'static str,
    pub unit: &'static str,
    pub amount: f64,
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestUsageRate {
    pub model: String,
    pub currency: String,
    pub input_per_million: f64,
    pub cached_input_per_million: f64,
    pub output_per_million: f64,
    pub basis: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EstimateStatus {
    Available,
    MissingRate,
    UnknownCache,
    UnknownTier,
    #[default]
    NoUsage,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRouteDay {
    pub date: String,
    pub agent: String,
    pub provider_id: String,
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credit_meter_id: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scoped_workbuddy_credits: Option<f64>,
    #[serde(flatten)]
    pub counts: UsageCounts,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsageDateRange {
    pub first: String,
    pub last: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsageHour {
    pub hour: u8,
    #[serde(flatten)]
    pub counts: UsageCounts,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub totals: UsageCounts,
    pub days: Vec<UsageDay>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hours: Option<Vec<UsageHour>>,
    pub by_agent: Vec<UsageGroup>,
    pub by_provider: Vec<UsageGroup>,
    pub by_model: Vec<UsageGroup>,
    /// 只统计填了价格的网关供应商。
    pub costs: Vec<UsageCost>,
    pub daily_routes: Vec<UsageRouteDay>,
    /// Bounds of observed rows, not a claim of uninterrupted capture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_date_range: Option<UsageDateRange>,
}

/// 本地日期（`YYYY-MM-DD`）。
pub fn local_day(timestamp_ms: u64) -> String {
    i64::try_from(timestamp_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|| "1970-01-01".to_string())
}

/// 日期加减天数。
pub fn shift_day(day: &str, delta: i64) -> String {
    rusqlite::Connection::open_in_memory()
        .and_then(|connection| {
            connection.query_row(
                "SELECT date(?1, ?2 || ' days')",
                rusqlite::params![day, delta],
                |row| row.get::<_, String>(0),
            )
        })
        .unwrap_or_else(|_| day.to_string())
}

/// 请求结束时记一笔（从请求日志条目取数）。失败不影响请求本身。
/// 在后台线程里记：请求结束时在本地服务的单线程运行时里调用，整份文件重写加 fsync 会卡住别的流。
pub fn record_entry(entry: &RequestLogEntry) {
    let entry = entry.clone();
    // 覆盖日志路径的测试必须同样隔离用量文件；在线程启动前捕获，避免测试 guard 已恢复。
    let store = crate::request_log::log_path_override()
        .map(|path| UsageStore::at(path.with_file_name(USAGE_FILE)))
        .unwrap_or_default();
    std::thread::spawn(move || {
        let _ = record_entry_in(&store, &entry);
    });
}

pub fn record_entry_in(store: &UsageStore, entry: &RequestLogEntry) -> anyhow::Result<()> {
    if entry.state.as_deref() == Some("running") {
        return Ok(());
    }
    let usage = entry.usage.clone().unwrap_or_default();
    let row = UsageRow {
        agent: entry.agent_id().to_string(),
        provider_id: entry.relay_id.clone(),
        provider: entry.relay_name.clone(),
        model: entry.usage_model().to_string(),
        counts: UsageCounts {
            calls: 1,
            errors: u64::from(!entry.success),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_tokens: usage.cached_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            duration_ms: entry.duration_ms,
            timed_calls: u64::from(entry.duration_ms > 0),
            images: u64::from(entry.success && entry.is_image()),
            workbuddy_credits: usage.workbuddy_credits,
            token_metered_calls: Some(u64::from(entry.usage.is_some())),
        },
        scoped_workbuddy_credits: workbuddy_meter_id(&entry.upstream_model)
            .and(usage.workbuddy_credits),
        extra: Default::default(),
    };
    let timestamp = if entry.timestamp_ms > 0 {
        entry.timestamp_ms
    } else {
        entry.started_at_ms
    };
    if let Some(date) = i64::try_from(timestamp)
        .ok()
        .filter(|timestamp| *timestamp > 0)
        .and_then(chrono::DateTime::from_timestamp_millis)
    {
        let local = date.with_timezone(&chrono::Local);
        let day = local.format("%Y-%m-%d").to_string();
        store.record_at(&day, Some(local.hour() as u8), row)
    } else {
        store.record(&local_day(timestamp), row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::store::GatewayProvider;
    use crate::request_log::RequestUsage;
    use crate::settings::RelayProfile;

    #[test]
    fn today_returns_observed_hours_without_allocating_legacy_daily_rows() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::at(dir.path().join(USAGE_FILE));
        let mut request = entry(Some("codex"), "relay", 30, 15, 10);
        let local_day = local_day(request.timestamp_ms);
        let local_hour = chrono::DateTime::from_timestamp_millis(request.timestamp_ms as i64)
            .unwrap()
            .with_timezone(&chrono::Local)
            .hour() as u8;
        let gateway = GatewaySettings::default();

        store
            .record(
                &local_day,
                UsageRow {
                    counts: UsageCounts {
                        calls: 1,
                        input_tokens: 20,
                        ..UsageCounts::default()
                    },
                    ..UsageRow::default()
                },
            )
            .unwrap();
        request.state = Some("running".into());
        record_entry_in(&store, &request).unwrap();
        request.state = None;
        record_entry_in(&store, &request).unwrap();
        record_entry_in(&store, &request).unwrap();

        let summary = store.summary(&local_day, Some(1), &gateway).unwrap();
        assert_eq!(summary.totals.calls, 3);
        let hours = summary.hours.unwrap();
        assert_eq!(hours.len(), 1);
        assert_eq!(hours[0].hour, local_hour);
        assert_eq!(hours[0].counts.calls, 2);
        assert_eq!(hours[0].counts.input_tokens, 60);
        assert!(
            store
                .summary(&local_day, Some(7), &gateway)
                .unwrap()
                .hours
                .is_none()
        );
        store.clear().unwrap();
        assert!(
            store
                .summary(&local_day, Some(1), &gateway)
                .unwrap()
                .hours
                .unwrap()
                .is_empty()
        );
    }

    fn entry(
        agent: Option<&str>,
        relay: &str,
        input: u64,
        output: u64,
        cached: u64,
    ) -> RequestLogEntry {
        RequestLogEntry {
            id: "id".to_string(),
            timestamp_ms: 1_790_000_000_000,
            started_at_ms: 0,
            duration_ms: 1200,
            method: "POST".to_string(),
            path: "/v1/messages".to_string(),
            status_code: 200,
            success: true,
            stream: true,
            model: "claude-sonnet-5".to_string(),
            thread_id: None,
            upstream_model: "deepseek-v4-pro".to_string(),
            relay_id: relay.to_string(),
            relay_name: relay.to_uppercase(),
            wire_api: "ChatCompletions".to_string(),
            attempts: 1,
            auto_retries: 0,
            retry_reasons: Vec::new(),
            attempt_details: Vec::new(),
            state: None,
            first_token_ms: None,
            estimated_output_tokens: None,
            phase: None,
            error: None,
            stream_outcome: None,
            usage: Some(RequestUsage {
                input_tokens: input,
                output_tokens: output,
                cached_tokens: cached,
                reasoning_tokens: 1,
                workbuddy_credits: None,
                cached_tokens_known: Some(true),
            }),
            cache_hit_ratio: None,
            context_summary: None,
            final_response_evidence: None,
            final_response_id: None,
            route: None,
            agent: agent.map(str::to_string),
        }
    }

    #[test]
    fn days_shift_and_format() {
        assert_eq!(shift_day("2026-10-01", -1), "2026-09-30");
        assert_eq!(shift_day("2026-12-31", 1), "2027-01-01");
        assert_eq!(local_day(0).len(), 10);
    }

    #[test]
    fn workbuddy_credits_follow_the_terminal_response_into_usage() {
        let mut transformer = crate::protocol_proxy::ResponsesSseTransformer::default();
        transformer.push_bytes(b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":12,\"output_tokens\":3,\"input_tokens_details\":{\"cached_tokens\":0},\"workbuddy_credits\":0.375}}}\n\n");
        let mut metered = entry(None, "wb", 12, 3, 0);
        metered.usage = transformer.take_usage();
        assert_eq!(
            metered.usage.as_ref().unwrap().workbuddy_credits,
            Some(0.375)
        );

        let temp = tempfile::tempdir().unwrap();
        let store = UsageStore::at(temp.path().join("usage.json"));
        let day = local_day(metered.timestamp_ms);
        let gateway = GatewaySettings::default();
        let old = entry(None, "wb", 10, 2, 0);
        record_entry_in(&store, &old).unwrap();
        assert!(entry_cost(&old, &gateway).is_none());
        assert_eq!(
            store
                .summary(&day, None, &gateway)
                .unwrap()
                .totals
                .workbuddy_credits,
            None
        );

        let mut free = metered.clone();
        free.usage.as_mut().unwrap().workbuddy_credits = Some(0.0);
        assert_eq!(entry_cost(&free, &gateway).unwrap().amount, 0.0);
        record_entry_in(&store, &free).unwrap();
        assert_eq!(
            store
                .summary(&day, None, &gateway)
                .unwrap()
                .totals
                .workbuddy_credits,
            Some(0.0)
        );

        record_entry_in(&store, &metered).unwrap();
        let cost = entry_cost(&metered, &gateway).unwrap();
        assert_eq!((cost.currency.as_str(), cost.amount), ("积分", 0.375));
        let summary = store.summary(&day, None, &gateway).unwrap();
        assert_eq!(summary.totals.workbuddy_credits, Some(0.375));
        assert_eq!(summary.days[0].counts.workbuddy_credits, Some(0.375));
        assert_eq!(summary.by_provider[0].counts.workbuddy_credits, Some(0.375));
        assert_eq!(summary.by_model[0].counts.workbuddy_credits, Some(0.375));
        assert_eq!(summary.by_agent[0].counts.workbuddy_credits, Some(0.375));
        assert!(summary.costs.is_empty());

        let mut legacy_scoped_name = free.clone();
        legacy_scoped_name.model = "global:deepseek-v4-pro".to_string();
        legacy_scoped_name.upstream_model.clear();
        legacy_scoped_name.usage.as_mut().unwrap().workbuddy_credits = Some(8.0);
        record_entry_in(&store, &legacy_scoped_name).unwrap();
        let mut proven_scope = metered.clone();
        proven_scope.upstream_model = "global:deepseek-v4-pro".to_string();
        record_entry_in(&store, &proven_scope).unwrap();
        let with_scope = store.summary(&day, None, &gateway).unwrap();
        let scoped_lane = with_scope
            .daily_routes
            .iter()
            .find(|route| route.model == "global:deepseek-v4-pro")
            .unwrap();
        assert_eq!(scoped_lane.credit_meter_id, Some("workbuddy:global"));
        assert_eq!(scoped_lane.scoped_workbuddy_credits, Some(0.375));
        assert_eq!(scoped_lane.counts.workbuddy_credits, Some(8.375));
    }

    #[test]
    fn records_accumulate_per_day_and_summarize_with_costs() {
        let temp = tempfile::tempdir().unwrap();
        let store = UsageStore::at(temp.path().join("usage.json"));
        record_entry_in(
            &store,
            &entry(Some("claude"), "ds", 1_000_000, 200_000, 400_000),
        )
        .unwrap();
        record_entry_in(&store, &entry(Some("claude"), "ds", 1_000_000, 0, 0)).unwrap();
        record_entry_in(&store, &entry(None, "codex-relay", 10, 5, 0)).unwrap();
        let mut failed = entry(Some("grok"), "ds", 0, 0, 0);
        failed.success = false;
        failed.usage = None;
        record_entry_in(&store, &failed).unwrap();
        let mut image = entry(None, "img", 0, 0, 0);
        image.wire_api = "imageGenerations".to_string();
        image.usage = None;
        record_entry_in(&store, &image).unwrap();

        let day = local_day(1_790_000_000_000);
        let mut gateway = GatewaySettings::default();
        gateway.providers.push(GatewayProvider {
            enabled: true,
            source: String::new(),
            model_prices: Default::default(),
            model_windows: Default::default(),
            context_window: None,
            proxy: String::new(),
            key_rpm: None,
            key_concurrency: None,
            price: Some(ProviderPrice {
                currency: "cny".to_string(),
                input: 2.0,
                cached_input: 0.5,
                output: 8.0,
            }),
            upstream_new: Vec::new(),
            upstream_gone: Vec::new(),
            models_checked_at_ms: 0,
            newapi_access: None,
            subscription_policy: None,
            upstream_meta: Default::default(),
            auto_compact: Default::default(),
            relay: RelayProfile {
                id: "ds".to_string(),
                relay_mode: crate::settings::RelayMode::PureApi,
                ..RelayProfile::default()
            },
            extra: Default::default(),
        });
        let summary = store.summary(&day, Some(7), &gateway).unwrap();
        assert_eq!(summary.days.len(), 7);
        assert_eq!(summary.days.last().unwrap().date, day);
        assert_eq!(summary.totals.calls, 5);
        assert_eq!(summary.totals.errors, 1);
        assert_eq!(summary.totals.images, 1, "成功的生图按次数记");
        assert_eq!(summary.totals.input_tokens, 2_000_010);
        assert_eq!(summary.totals.cached_tokens, 400_000);
        let agents = summary
            .by_agent
            .iter()
            .map(|group| (group.key.as_str(), group.counts.calls))
            .collect::<Vec<_>>();
        assert_eq!(agents, vec![("claude", 2), ("codex", 2), ("grok", 1)]);
        // (2M - 0.4M) × 2 + 0.4M × 0.5 + 0.2M × 8 = 3.2 + 0.2 + 1.6 = 5.0 元
        assert_eq!(summary.costs.len(), 1);
        assert_eq!(summary.costs[0].currency, "CNY");
        assert!((summary.costs[0].amount - 5.0).abs() < 1e-9);
        let ds = summary
            .by_provider
            .iter()
            .find(|group| group.key == "ds")
            .unwrap();
        assert!((ds.costs[0].amount - 5.0).abs() < 1e-9, "每组也算出费用");
        assert!(
            summary
                .by_provider
                .iter()
                .find(|group| group.key == "codex-relay")
                .unwrap()
                .costs
                .is_empty()
        );
        assert_eq!(summary.totals.timed_calls, 5);
        assert_eq!(summary.totals.duration_ms, 5 * 1200);
        assert_eq!(summary.totals.token_metered_calls, Some(3));
        assert_eq!(summary.daily_routes.len(), 4);
        assert!(summary.daily_routes.iter().all(|route| route.date == day));
        let joint = summary
            .daily_routes
            .iter()
            .find(|route| route.agent == "claude")
            .unwrap();
        assert_eq!(
            (joint.provider_id.as_str(), joint.model.as_str()),
            ("ds", "deepseek-v4-pro")
        );
        assert_eq!(joint.counts.calls, 2);
        assert_eq!(joint.counts.token_metered_calls, Some(2));
        assert_eq!(
            summary.observed_date_range,
            Some(UsageDateRange {
                first: day.clone(),
                last: day.clone()
            })
        );

        // 每条请求的费用按同样的价格算：1M 输入（无缓存）× 2 = 2 元。
        let cost = entry_cost(&entry(Some("claude"), "ds", 1_000_000, 0, 0), &gateway).unwrap();
        assert_eq!(cost.currency, "CNY");
        assert!((cost.amount - 2.0).abs() < 1e-9);
        let mut subscription_gateway = gateway.clone();
        subscription_gateway.providers[0].relay.relay_mode = crate::settings::RelayMode::Official;
        let subscription = entry_pricing(
            &entry(Some("claude"), "ds", 1_000_000, 0, 0),
            &subscription_gateway,
        );
        assert!(subscription.rate.is_some());
        assert!(subscription.estimate.is_none());
        assert_eq!(subscription.estimate_status, EstimateStatus::UnknownTier);
        assert!(
            entry_cost(&entry(None, "codex-relay", 10, 5, 0), &gateway).is_none(),
            "没填价格不算"
        );

        let mut unreported_cache = entry(Some("claude"), "ds", 12, 0, 0);
        unreported_cache.usage.as_mut().unwrap().cached_tokens_known = Some(false);
        let unknown = entry_pricing(&unreported_cache, &gateway);
        assert!(
            unknown.rate.is_some(),
            "tariff is independent of cache telemetry"
        );
        assert!(unknown.estimate.is_none());
        assert_eq!(unknown.estimate_status, EstimateStatus::UnknownCache);
        assert!(entry_cost(&unreported_cache, &gateway).is_none());

        let mut actual = entry(Some("claude"), "ds", 0, 0, 0);
        actual.upstream_model = "global:deepseek-v4-pro".to_string();
        actual.usage.as_mut().unwrap().workbuddy_credits = Some(0.0);
        let priced = entry_pricing(&actual, &gateway);
        assert_eq!(priced.actual_meters[0].meter_id, "workbuddy:global");
        assert_eq!(priced.actual_meters[0].amount, 0.0);
        assert_eq!(priced.estimate_status, EstimateStatus::Available);
        assert_eq!(
            priced.estimate,
            Some(UsageCost {
                currency: "CNY".into(),
                amount: 0.0
            })
        );

        actual.model = "global:deepseek-v4-pro".to_string();
        actual.upstream_model.clear();
        actual.usage.as_mut().unwrap().workbuddy_credits = Some(2.0);
        let unclassified = entry_pricing(&actual, &gateway);
        assert_eq!(
            unclassified.actual_meters[0].meter_id,
            "workbuddy:unclassified"
        );
        assert_eq!(unclassified.estimate_status, EstimateStatus::MissingRate);

        let earlier = shift_day(&day, -2);
        store
            .record(
                &earlier,
                UsageRow {
                    agent: "claude".into(),
                    provider_id: "ds".into(),
                    provider: "DS".into(),
                    model: "old".into(),
                    counts: UsageCounts {
                        calls: 3,
                        input_tokens: 20,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        let with_history = store.summary(&day, Some(7), &gateway).unwrap();
        assert_eq!(
            with_history.daily_routes[0].counts.token_metered_calls,
            None
        );
        assert_eq!(with_history.totals.token_metered_calls, Some(3));
        assert_eq!(with_history.totals.calls, 8);
        assert_eq!(with_history.observed_date_range.unwrap().first, earlier);

        let all = store.summary(&day, None, &gateway).unwrap();
        assert_eq!(all.days.len(), 3);
        store.clear().unwrap();
        assert_eq!(store.summary(&day, None, &gateway).unwrap().totals.calls, 0);
    }

    /// 更新版本加的统计项、顶层字段：这一版继续记账时原样保留。
    #[test]
    fn unknown_fields_survive_recording() {
        let temp = tempfile::tempdir().unwrap();
        let store = UsageStore::at(temp.path().join("gateway-usage.json"));
        let day = local_day(1_790_000_000_000);
        let text = serde_json::json!({
            "version": 9,
            "futureTop": "keep",
            "days": { day.clone(): [{
                "agent": "claude", "providerId": "ds", "provider": "DS",
                "model": "deepseek-v4-pro", "calls": 2, "cacheWriteTokens": 77
            }]}
        });
        std::fs::write(&store.path, text.to_string()).unwrap();
        record_entry_in(&store, &entry(Some("claude"), "ds", 10, 5, 0)).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&store.path).unwrap()).unwrap();
        assert_eq!(saved["futureTop"], "keep");
        let row = &saved["days"][&day][0];
        assert_eq!(row["calls"], 3);
        assert_eq!(row["cacheWriteTokens"], 77);
    }
}
