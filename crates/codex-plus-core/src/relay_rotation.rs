/**
 * @description 聚合供应商轮转选择器，负责按失败、对话、请求和权重策略选择已有中转配置。
 * @author Albert_Luo
 * @email 480199976@qq.com
 * @date 2026-05-27 00:00
 */
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::json;

use crate::diagnostic_log::append_diagnostic_log;
use crate::settings::{
    AggregateRelayProfile, AggregateRelayRoute, AggregateRelayStrategy, BackendSettings,
    RelayProfile,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionError {
    NoActiveAggregate,
    EmptyAggregateMembers {
        aggregate_id: String,
    },
    UnsupportedModel {
        aggregate_id: String,
        model: String,
    },
    UnknownMemberRelay {
        aggregate_id: String,
        relay_id: String,
    },
    InvalidMemberRelay {
        aggregate_id: String,
        relay_id: String,
    },
    ExcludedMemberRelay {
        aggregate_id: String,
        relay_id: String,
    },
    UnknownDedicatedRelayModel {
        model: String,
    },
}

impl std::fmt::Display for SelectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectionError::NoActiveAggregate => write!(formatter, "未找到当前聚合供应商"),
            SelectionError::EmptyAggregateMembers { aggregate_id } => {
                write!(formatter, "聚合供应商「{aggregate_id}」没有成员")
            }
            SelectionError::UnsupportedModel {
                aggregate_id,
                model,
            } => write!(
                formatter,
                "聚合供应商「{aggregate_id}」中没有可以处理模型「{model}」的成员"
            ),
            SelectionError::UnknownMemberRelay {
                aggregate_id,
                relay_id,
            } => write!(
                formatter,
                "聚合供应商「{aggregate_id}」引用了不存在的供应商「{relay_id}」"
            ),
            SelectionError::InvalidMemberRelay {
                aggregate_id,
                relay_id,
            } => write!(
                formatter,
                "聚合供应商「{aggregate_id}」成员「{relay_id}」缺少 API Base URL 或 Key"
            ),
            SelectionError::ExcludedMemberRelay {
                aggregate_id,
                relay_id,
            } => write!(
                formatter,
                "聚合供应商「{aggregate_id}」不得调度专用直连供应商「{relay_id}」"
            ),
            SelectionError::UnknownDedicatedRelayModel { model } => {
                write!(formatter, "未找到专用 API 模型「{model}」")
            }
        }
    }
}

impl std::error::Error for SelectionError {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RotationContext {
    pub conversation_id: Option<String>,
    pub model: Option<String>,
}

impl RotationContext {
    pub fn for_conversation(conversation_id: impl Into<String>) -> Self {
        Self {
            conversation_id: Some(conversation_id.into()),
            model: None,
        }
    }
}

/// 路由匹配过程结果信息，供 protocol_proxy 记录 diagnostic_log
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteMatchInfo {
    /// 规则命中且目标成员有效（含目标 relayId 与规则 pattern）
    Matched { relay_id: String, pattern: String },
    /// 规则命中但目标成员无效（不存在或缺少 base_url/api_key），已跳过
    SkippedInvalidRelay { relay_id: String, pattern: String },
    /// 路由未命中（仅当 routes 非空且 model 存在时产生）
    Unmatched,
}

/// 通配符匹配：pattern 与 model 均 trim + 忽略大小写；`*` 匹配任意 0+ 字符；
/// 无 `*` 时精确匹配；含 `*` 时按 `*` 分段，各段在 model 中按序且不重叠出现
pub fn match_route_pattern(pattern: &str, model: &str) -> bool {
    let pattern = pattern.trim().to_lowercase();
    let model = model.trim().to_lowercase();
    if pattern.is_empty() {
        return false;
    }
    if !pattern.contains('*') {
        return pattern == model;
    }
    let segments: Vec<&str> = pattern.split('*').collect();
    let mut position = 0usize;
    for (index, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            continue;
        }
        if index == 0 {
            // 首段必须是前缀
            if !model[position..].starts_with(segment) {
                return false;
            }
            position += segment.len();
        } else if index == segments.len() - 1 {
            // 末段必须是后缀，且不能与已匹配部分重叠
            if !model.ends_with(segment) || model.len() - segment.len() < position {
                return false;
            }
            position = model.len();
        } else {
            // 中间段按序出现
            let Some(relative) = model[position..].find(segment) else {
                return false;
            };
            position += relative + segment.len();
        }
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationEvent {
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixedModelRoute {
    Official,
    DedicatedRelay,
    Aggregate,
    Reject,
}

pub fn classify_mixed_model_route(
    settings: &BackendSettings,
    model: Option<&str>,
) -> MixedModelRoute {
    if settings.active_aggregate_relay_profile().is_none() {
        return MixedModelRoute::Aggregate;
    }
    let official_auth_first = settings.active_relay_uses_official_login_auth();
    let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
        return if official_auth_first {
            MixedModelRoute::Reject
        } else {
            MixedModelRoute::Aggregate
        };
    };
    if official_auth_first
        && crate::aggregate_model_alias::is_trusted_official_codex_model_for_settings(
            model, settings,
        )
    {
        return MixedModelRoute::Official;
    }
    if crate::aggregate_model_alias::cliproxy_direct_api_aliases_for_settings(
        &settings.relay_profiles,
        settings,
    )
    .iter()
    .any(|alias| alias.alias.eq_ignore_ascii_case(model))
    {
        return MixedModelRoute::DedicatedRelay;
    }

    let Some(aggregate) = settings.active_aggregate_relay_profile() else {
        return MixedModelRoute::Reject;
    };
    let members = aggregate
        .members
        .iter()
        .filter_map(|member| raw_relay_profile_by_id(settings, &member.relay_id).cloned())
        .collect::<Vec<_>>();
    let is_replacement =
        crate::aggregate_model_alias::aggregate_replacement_model_aliases(&aggregate, &members)
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(model));
    let aliases = crate::aggregate_model_alias::aggregate_catalog_aliases(&aggregate, &members);
    let is_provider_alias = aliases.iter().any(|alias| {
        alias.alias.contains(':')
            && !alias.alias.contains('(')
            && alias.alias.eq_ignore_ascii_case(model)
    });
    if is_replacement || is_provider_alias {
        MixedModelRoute::Aggregate
    } else if official_auth_first {
        MixedModelRoute::Reject
    } else {
        MixedModelRoute::Aggregate
    }
}

pub fn select_dedicated_relay_for_model(
    settings: &BackendSettings,
    model: Option<&str>,
) -> Result<RelayProfile, SelectionError> {
    let model = model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| SelectionError::UnknownDedicatedRelayModel {
            model: String::new(),
        })?;
    let alias = crate::aggregate_model_alias::cliproxy_direct_api_aliases_for_settings(
        &settings.relay_profiles,
        settings,
    )
    .into_iter()
    .find(|alias| alias.alias.eq_ignore_ascii_case(model))
    .ok_or_else(|| SelectionError::UnknownDedicatedRelayModel {
        model: model.to_string(),
    })?;
    let mut relay = raw_relay_profile_by_id(settings, &alias.relay_id)
        .cloned()
        .ok_or_else(|| SelectionError::UnknownDedicatedRelayModel {
            model: model.to_string(),
        })?;
    relay.model_mappings.insert(alias.alias, alias.target_model);
    relay.model_mappings_enabled = true;
    Ok(relay)
}

#[derive(Debug, Clone)]
pub struct RelayRotationSelector {
    aggregate: AggregateRelayProfile,
    failover_index: usize,
    request_index: usize,
    weighted_index: usize,
    last_member_pool_signature: Vec<String>,
    conversation_assignments: HashMap<String, String>,
}

static GLOBAL_SELECTOR: OnceLock<Mutex<Option<RelayRotationSelector>>> = OnceLock::new();

impl RelayRotationSelector {
    pub fn from_settings(settings: &BackendSettings) -> Result<Self, SelectionError> {
        let aggregate = active_aggregate(settings)?.clone();
        ensure_aggregate_has_members(&aggregate)?;
        validate_excluded_members(settings, &aggregate)?;
        Ok(Self {
            aggregate,
            failover_index: 0,
            request_index: 0,
            weighted_index: 0,
            last_member_pool_signature: Vec::new(),
            conversation_assignments: HashMap::new(),
        })
    }

    pub fn select(
        &mut self,
        settings: &BackendSettings,
        context: RotationContext,
        model: Option<&str>,
    ) -> Result<RelayProfile, SelectionError> {
        self.select_with_outcomes(settings, context, model)
            .map(|(relay, _outcomes)| relay)
    }

    /// 选择 relay 的同时返回路由匹配过程结果，供调用方记录 diagnostic_log。
    /// 保证日志与实际选择来自同一次匹配，避免二次匹配带来的结果与日志不一致
    pub fn select_with_outcomes(
        &mut self,
        settings: &BackendSettings,
        context: RotationContext,
        model: Option<&str>,
    ) -> Result<(RelayProfile, Vec<RouteMatchInfo>), SelectionError> {
        let requested_model = model.or(context.model.as_deref()).map(str::to_string);
        let (routed_relay_id, outcomes) =
            match_route_for_aggregate(settings, &self.aggregate, requested_model.as_deref());
        let relay = if let Some(relay_id) = routed_relay_id {
            // 路由目标已由 relay_is_available 保证有效，直接取用
            relay_profile_by_id(settings, &relay_id).ok_or_else(|| {
                SelectionError::UnknownMemberRelay {
                    aggregate_id: self.aggregate.id.clone(),
                    relay_id,
                }
            })?
        } else {
            let members =
                member_pool_for_model(settings, &self.aggregate, requested_model.as_deref())?;
            self.refresh_pool_state(&members);
            let relay_id = match self.aggregate.strategy {
                AggregateRelayStrategy::Failover => member_id_at(&members, self.failover_index),
                AggregateRelayStrategy::ConversationRoundRobin => {
                    self.select_for_conversation(context.conversation_id, &members)
                }
                AggregateRelayStrategy::RequestRoundRobin => self.select_next_request(&members),
                AggregateRelayStrategy::WeightedRoundRobin => self.select_next_weighted(&members),
            };
            selected_member_relay(settings, &self.aggregate, &relay_id)?
        };
        Ok((relay, outcomes))
    }

    pub fn peek(
        &self,
        settings: &BackendSettings,
        model: Option<&str>,
    ) -> Result<RelayProfile, SelectionError> {
        ensure_aggregate_has_members(&self.aggregate)?;
        let members = member_pool_for_model(settings, &self.aggregate, model)?;
        let relay_id = match self.aggregate.strategy {
            AggregateRelayStrategy::Failover => member_id_at(&members, self.failover_index),
            AggregateRelayStrategy::ConversationRoundRobin
            | AggregateRelayStrategy::RequestRoundRobin => {
                member_id_at(&members, self.request_index)
            }
            AggregateRelayStrategy::WeightedRoundRobin => {
                let schedule = weighted_schedule(&members);
                schedule[self.weighted_index % schedule.len()].clone()
            }
        };
        selected_member_relay(settings, &self.aggregate, &relay_id)
    }

    pub fn record_event(&mut self, event: RotationEvent) {
        if event == RotationEvent::Failure
            && self.aggregate.strategy == AggregateRelayStrategy::Failover
            && !self.aggregate.members.is_empty()
        {
            self.failover_index = (self.failover_index + 1) % self.aggregate.members.len();
        }
    }

    fn select_for_conversation(
        &mut self,
        conversation_id: Option<String>,
        members: &[crate::settings::AggregateRelayMember],
    ) -> String {
        let Some(conversation_id) = conversation_id else {
            return self.select_next_request(members);
        };
        if let Some(relay_id) = self.conversation_assignments.get(&conversation_id) {
            if members.iter().any(|member| member.relay_id == *relay_id) {
                return relay_id.clone();
            }
        }

        let relay_id = self.select_next_request(members);
        self.conversation_assignments
            .insert(conversation_id, relay_id.clone());
        relay_id
    }

    fn select_next_request(&mut self, members: &[crate::settings::AggregateRelayMember]) -> String {
        let relay_id = member_id_at(members, self.request_index);
        self.request_index = (self.request_index + 1) % members.len();
        relay_id
    }

    fn select_next_weighted(
        &mut self,
        members: &[crate::settings::AggregateRelayMember],
    ) -> String {
        let schedule = weighted_schedule(members);
        let relay_id = schedule[self.weighted_index % schedule.len()].clone();
        self.weighted_index = (self.weighted_index + 1) % schedule.len();
        relay_id
    }

    fn refresh_pool_state(&mut self, members: &[crate::settings::AggregateRelayMember]) {
        let signature = members
            .iter()
            .map(|member| format!("{}:{}", member.relay_id, member.weight))
            .collect::<Vec<_>>();
        if self.last_member_pool_signature != signature {
            self.request_index = 0;
            self.weighted_index = 0;
            self.last_member_pool_signature = signature;
        }
    }
}

pub fn select_relay_for_request(
    settings: &BackendSettings,
    context: RotationContext,
    model: Option<&str>,
) -> Result<RelayProfile, SelectionError> {
    let Some(active_aggregate) = settings.active_aggregate_relay_profile() else {
        clear_global_selector();
        return Ok(settings.active_relay_profile());
    };

    let lock = GLOBAL_SELECTOR.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let needs_new_selector = guard
        .as_ref()
        .map(|selector| selector.aggregate != active_aggregate)
        .unwrap_or(true);
    if needs_new_selector {
        *guard = Some(RelayRotationSelector::from_settings(settings)?);
    }
    let requested_model = model.map(str::to_string).or_else(|| context.model.clone());
    let (relay, outcomes) = guard
        .as_mut()
        .expect("selector initialized")
        .select_with_outcomes(settings, context, requested_model.as_deref())?;
    log_route_outcomes(requested_model.as_deref(), &outcomes);
    Ok(relay)
}

/// 将路由匹配过程结果写入 diagnostic_log，事件名与字段与旧 protocol_proxy 侧实现完全一致；
/// 仅在聚合选择路径被调用（无活动聚合时由调用方直接返回单 relay，不产生路由日志）
fn log_route_outcomes(model: Option<&str>, outcomes: &[RouteMatchInfo]) {
    for outcome in outcomes {
        let (event, detail) = match outcome {
            RouteMatchInfo::Matched { relay_id, pattern } => (
                "protocol_proxy.route_matched",
                json!({
                    "relayId": relay_id,
                    "model": model,
                    "rule": pattern,
                }),
            ),
            RouteMatchInfo::SkippedInvalidRelay { relay_id, pattern } => (
                "protocol_proxy.route_skipped_invalid_relay",
                json!({
                    "relayId": relay_id,
                    "model": model,
                    "rule": pattern,
                }),
            ),
            RouteMatchInfo::Unmatched => (
                "protocol_proxy.route_unmatched",
                json!({
                    "model": model,
                }),
            ),
        };
        let _ = append_diagnostic_log(event, detail);
    }
}

pub fn select_relay_for_probe(
    settings: &BackendSettings,
    model: Option<&str>,
) -> Result<RelayProfile, SelectionError> {
    let Some(active_aggregate) = settings.active_aggregate_relay_profile() else {
        clear_global_selector();
        return Ok(settings.active_relay_profile());
    };

    let lock = GLOBAL_SELECTOR.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let needs_new_selector = guard
        .as_ref()
        .map(|selector| selector.aggregate != active_aggregate)
        .unwrap_or(true);
    if needs_new_selector {
        *guard = Some(RelayRotationSelector::from_settings(settings)?);
    }
    guard
        .as_ref()
        .expect("selector initialized")
        .peek(settings, model)
}

pub fn fallback_relays_after(
    settings: &BackendSettings,
    relay_id: &str,
    model: Option<&str>,
) -> Result<Vec<RelayProfile>, SelectionError> {
    let Some(active_aggregate) = settings.active_aggregate_relay_profile() else {
        return Ok(Vec::new());
    };
    ensure_aggregate_has_members(&active_aggregate)?;
    let members = member_pool_for_model(settings, &active_aggregate, model)?;
    let selected_index = members
        .iter()
        .position(|member| member.relay_id == relay_id);
    let start_index = selected_index.map(|index| index + 1).unwrap_or(0);
    let fallback_count = if selected_index.is_some() {
        members.len().saturating_sub(1)
    } else {
        members.len()
    };
    let fallbacks = (0..fallback_count)
        .map(|offset| {
            let index = (start_index + offset) % members.len();
            &members[index]
        })
        .filter(|member| relay_is_available(settings, &active_aggregate, &member.relay_id))
        .filter_map(|member| relay_profile_by_id(settings, &member.relay_id))
        .collect::<Vec<_>>();
    Ok(fallbacks)
}

pub fn record_relay_request_event(settings: &BackendSettings, event: RotationEvent) {
    if settings.active_aggregate_relay_profile().is_none() {
        clear_global_selector();
        return;
    }
    let lock = GLOBAL_SELECTOR.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(selector) = guard.as_mut() {
        selector.record_event(event);
    }
}

pub fn record_relay_request_failure(settings: &BackendSettings) {
    record_relay_request_event(settings, RotationEvent::Failure);
}

fn active_aggregate(settings: &BackendSettings) -> Result<&AggregateRelayProfile, SelectionError> {
    let active_id = settings
        .active_aggregate_relay_profile()
        .map(|aggregate| aggregate.id)
        .ok_or(SelectionError::NoActiveAggregate)?;

    settings
        .aggregate_relay_profiles
        .iter()
        .find(|aggregate| aggregate.id == active_id)
        .ok_or(SelectionError::NoActiveAggregate)
}

fn ensure_aggregate_has_members(aggregate: &AggregateRelayProfile) -> Result<(), SelectionError> {
    if aggregate.members.is_empty() {
        return Err(SelectionError::EmptyAggregateMembers {
            aggregate_id: aggregate.id.clone(),
        });
    }
    Ok(())
}

fn validate_excluded_members(
    settings: &BackendSettings,
    aggregate: &AggregateRelayProfile,
) -> Result<(), SelectionError> {
    for member in &aggregate.members {
        let Some(relay) = raw_relay_profile_by_id(settings, &member.relay_id) else {
            continue;
        };
        if crate::aggregate_model_alias::integration_is_excluded_from_aggregate(
            &relay.integration_type,
        ) {
            return Err(SelectionError::ExcludedMemberRelay {
                aggregate_id: aggregate.id.clone(),
                relay_id: member.relay_id.clone(),
            });
        }
    }
    Ok(())
}

/// 最终选中的普通策略成员严格校验：relay 必须存在且 base_url/api_key 非空
fn selected_member_relay(
    settings: &BackendSettings,
    aggregate: &AggregateRelayProfile,
    relay_id: &str,
) -> Result<RelayProfile, SelectionError> {
    let relay = relay_profile_by_id(settings, relay_id).ok_or_else(|| {
        SelectionError::UnknownMemberRelay {
            aggregate_id: aggregate.id.clone(),
            relay_id: relay_id.to_string(),
        }
    })?;
    if crate::aggregate_model_alias::integration_is_excluded_from_aggregate(&relay.integration_type)
    {
        return Err(SelectionError::ExcludedMemberRelay {
            aggregate_id: aggregate.id.clone(),
            relay_id: relay_id.to_string(),
        });
    }
    if relay.base_url.trim().is_empty()
        || (relay.api_key.trim().is_empty() && !relay.uses_no_auth())
    {
        return Err(SelectionError::InvalidMemberRelay {
            aggregate_id: aggregate.id.clone(),
            relay_id: relay_id.to_string(),
        });
    }
    Ok(relay)
}

/// 按 model 执行聚合路由规则匹配，返回命中的成员 relayId 与过程结果信息
fn match_route_for_aggregate(
    settings: &BackendSettings,
    aggregate: &AggregateRelayProfile,
    model: Option<&str>,
) -> (Option<String>, Vec<RouteMatchInfo>) {
    let mut outcomes = Vec::new();
    if aggregate.routes.is_empty() {
        return (None, outcomes);
    }
    let Some(model) = model.map(str::trim).filter(|value| !value.is_empty()) else {
        return (None, outcomes);
    };
    let mut routes: Vec<&AggregateRelayRoute> = aggregate.routes.iter().collect();
    // 按 priority 降序；稳定排序保证同 priority 保持数组顺序
    routes.sort_by(|left, right| right.priority.cmp(&left.priority));
    for route in routes {
        if route.pattern.trim().is_empty() {
            continue;
        }
        if !match_route_pattern(&route.pattern, model) {
            continue;
        }
        if relay_is_available(settings, aggregate, &route.relay_id) {
            outcomes.push(RouteMatchInfo::Matched {
                relay_id: route.relay_id.clone(),
                pattern: route.pattern.clone(),
            });
            return (Some(route.relay_id.clone()), outcomes);
        }
        outcomes.push(RouteMatchInfo::SkippedInvalidRelay {
            relay_id: route.relay_id.clone(),
            pattern: route.pattern.clone(),
        });
    }
    outcomes.push(RouteMatchInfo::Unmatched);
    (None, outcomes)
}

/// 返回当前活动聚合的路由匹配过程结果。
/// 已不再被生产代码调用：路由日志已并入 select_with_outcomes（与实际选择同一次匹配），
/// 本函数仅保留供测试直接断言路由过程结果；无活动聚合或 model 缺失时为空
#[allow(dead_code)]
pub fn route_match_outcomes(
    settings: &BackendSettings,
    model: Option<&str>,
) -> Vec<RouteMatchInfo> {
    let Some(aggregate) = settings.active_aggregate_relay_profile() else {
        return Vec::new();
    };
    match_route_for_aggregate(settings, &aggregate, model).1
}

/// 路由目标有效性：属于聚合成员且 relay 存在、base_url/api_key 非空。
/// 除与 validate_aggregate_members 同标准外，额外要求目标必须在 aggregate.members 内，
/// 避免路由把请求导向聚合外部的独立 relay
fn relay_is_available(
    settings: &BackendSettings,
    aggregate: &AggregateRelayProfile,
    relay_id: &str,
) -> bool {
    if !aggregate
        .members
        .iter()
        .any(|member| member.relay_id == relay_id)
    {
        return false;
    }
    settings
        .relay_profiles
        .iter()
        .find(|profile| profile.id == relay_id)
        .map(|profile| {
            !crate::aggregate_model_alias::integration_is_excluded_from_aggregate(
                &profile.integration_type,
            ) && !profile.base_url.trim().is_empty()
                && (!profile.api_key.trim().is_empty() || profile.uses_no_auth())
        })
        .unwrap_or(false)
}

fn member_pool_for_model(
    settings: &BackendSettings,
    aggregate: &AggregateRelayProfile,
    model: Option<&str>,
) -> Result<Vec<crate::settings::AggregateRelayMember>, SelectionError> {
    let Some(model) = model.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(aggregate.members.clone());
    };
    if let Some(provider_specific_members) =
        aggregate_member_pool_for_provider_alias(settings, aggregate, model)
    {
        return Ok(provider_specific_members);
    }

    let explicit_alias_members = aggregate_dispatch_member_pool(settings, aggregate, model);
    if !explicit_alias_members.is_empty() {
        return Ok(explicit_alias_members);
    }

    let normalized_model = crate::aggregate_model_alias::normalize_requested_model_name(model);
    let model = if normalized_model.is_empty() {
        model
    } else {
        normalized_model.as_str()
    };

    let explicit_dispatch_members = aggregate_dispatch_member_pool(settings, aggregate, model);
    if !explicit_dispatch_members.is_empty() {
        return Ok(explicit_dispatch_members);
    }

    let direct_members = aggregate
        .members
        .iter()
        .filter(|member| {
            raw_relay_profile_by_id(settings, &member.relay_id)
                .is_some_and(|relay| relay_supports_direct_model(relay, model))
        })
        .cloned()
        .collect::<Vec<_>>();
    if !direct_members.is_empty() {
        return Ok(direct_members);
    }

    // 未声明模型目录或映射时，保持聚合的既有兜底语义：让成员自行决定
    // 是否接受该模型，而不是因 catalog 不完整阻断整个请求。
    Ok(aggregate.members.clone())
}

fn aggregate_member_pool_for_provider_alias(
    settings: &BackendSettings,
    aggregate: &AggregateRelayProfile,
    model: &str,
) -> Option<Vec<crate::settings::AggregateRelayMember>> {
    let member_profiles = aggregate
        .members
        .iter()
        .filter_map(|member| raw_relay_profile_by_id(settings, &member.relay_id).cloned())
        .collect::<Vec<_>>();
    let relay_weight_by_id = aggregate
        .members
        .iter()
        .map(|member| (member.relay_id.as_str(), member.weight))
        .collect::<HashMap<_, _>>();
    let mut matches = Vec::new();
    for profile in member_profiles {
        for alias in
            crate::aggregate_model_alias::aggregate_model_aliases_for_member(&profile, false)
        {
            if alias.alias != model {
                continue;
            }
            let relay_id = alias.provider_id;
            let weight = *relay_weight_by_id.get(relay_id.as_str()).unwrap_or(&1);
            matches.push(crate::settings::AggregateRelayMember { relay_id, weight });
        }
    }
    (!matches.is_empty()).then_some(matches)
}

fn aggregate_dispatch_member_pool(
    settings: &BackendSettings,
    aggregate: &AggregateRelayProfile,
    model: &str,
) -> Vec<crate::settings::AggregateRelayMember> {
    let member_profiles = aggregate
        .members
        .iter()
        .filter_map(|member| raw_relay_profile_by_id(settings, &member.relay_id).cloned())
        .collect::<Vec<_>>();
    let dispatch_entries =
        crate::aggregate_model_alias::aggregate_dispatch_entries(aggregate, &member_profiles);
    let relay_weight_by_id = aggregate
        .members
        .iter()
        .map(|member| (member.relay_id.as_str(), member.weight))
        .collect::<HashMap<_, _>>();

    dispatch_entries
        .into_iter()
        .filter(|entry| {
            entry.codex_model == model
                || entry.alias == model
                || crate::aggregate_model_alias::provider_label(
                    &entry.provider_name,
                    &entry.target_model,
                ) == model
        })
        .map(|entry| {
            let relay_id = entry.provider_id;
            let weight = *relay_weight_by_id.get(relay_id.as_str()).unwrap_or(&1);
            crate::settings::AggregateRelayMember { relay_id, weight }
        })
        .collect()
}

fn relay_supports_direct_model(relay: &RelayProfile, model: &str) -> bool {
    relay_models(relay).iter().any(|item| item == model)
        || crate::aggregate_model_alias::aggregate_model_aliases_for_member(relay, false)
            .iter()
            .any(|alias| alias.alias == model)
}

fn relay_models(relay: &RelayProfile) -> Vec<String> {
    crate::aggregate_model_alias::relay_profile_model_ids(relay)
}

fn weighted_schedule(members: &[crate::settings::AggregateRelayMember]) -> Vec<String> {
    members
        .iter()
        .flat_map(|member| {
            std::iter::repeat_n(member.relay_id.clone(), member.weight.max(1) as usize)
        })
        .collect()
}

fn member_id_at(members: &[crate::settings::AggregateRelayMember], index: usize) -> String {
    members[index % members.len()].relay_id.clone()
}

fn clear_global_selector() {
    let lock = GLOBAL_SELECTOR.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = None;
}

pub fn clear_relay_rotation_state_for_tests() {
    clear_global_selector();
}

fn raw_relay_profile_by_id<'a>(
    settings: &'a BackendSettings,
    relay_id: &str,
) -> Option<&'a RelayProfile> {
    settings
        .relay_profiles
        .iter()
        .find(|profile| profile.id == relay_id)
}

fn relay_profile_by_id(settings: &BackendSettings, relay_id: &str) -> Option<RelayProfile> {
    raw_relay_profile_by_id(settings, relay_id).map(|profile| {
        let mut relay = profile.clone();
        let mut effective_model_mappings = HashMap::new();

        for alias in
            crate::aggregate_model_alias::aggregate_model_aliases_for_member(profile, false)
        {
            effective_model_mappings.insert(alias.alias, alias.target_model);
        }

        if profile.model_mappings_enabled {
            for (mapping_key, target_model) in &profile.model_mappings {
                let mapping_key = mapping_key.trim();
                let target_model = target_model.trim();
                if mapping_key.is_empty() || target_model.is_empty() {
                    continue;
                }
                effective_model_mappings.insert(mapping_key.to_string(), target_model.to_string());
            }
            for alias in
                crate::aggregate_model_alias::aggregate_model_aliases_for_member(profile, true)
                    .into_iter()
                    .filter(|alias| alias.via_mapping)
            {
                effective_model_mappings.insert(alias.alias, alias.target_model);
            }
        }
        relay.model_list =
            crate::aggregate_model_alias::aggregate_model_aliases_for_member(profile, false)
                .into_iter()
                .map(|alias| alias.alias)
                .collect::<Vec<_>>()
                .join("\n");

        if let Some(aggregate) = settings.active_aggregate_relay_profile() {
            let member_profiles = aggregate
                .members
                .iter()
                .filter_map(|member| raw_relay_profile_by_id(settings, &member.relay_id).cloned())
                .collect::<Vec<_>>();
            for entry in crate::aggregate_model_alias::aggregate_dispatch_entries(
                &aggregate,
                &member_profiles,
            ) {
                if entry.provider_id == relay.id {
                    let target_model = entry.target_model;
                    effective_model_mappings.insert(entry.codex_model, target_model.clone());
                    effective_model_mappings.insert(
                        crate::aggregate_model_alias::provider_label(
                            &entry.provider_name,
                            &target_model,
                        ),
                        target_model,
                    );
                }
            }
        }

        relay.model_mappings = effective_model_mappings;
        relay.model_mappings_enabled = !relay.model_mappings.is_empty();

        relay
    })
}
