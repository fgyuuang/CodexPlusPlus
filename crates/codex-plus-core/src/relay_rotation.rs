/**
 * @description 聚合供应商轮转选择器，负责按失败、对话、请求和权重策略选择已有中转配置。
 * @author Albert_Luo
 * @email 480199976@qq.com
 * @date 2026-05-27 00:00
 */
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::settings::{
    AggregateRelayProfile, AggregateRelayStrategy, BackendSettings, RelayProfile,
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
        }
    }
}

impl std::error::Error for SelectionError {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RotationContext {
    pub conversation_id: Option<String>,
}

impl RotationContext {
    pub fn for_conversation(conversation_id: impl Into<String>) -> Self {
        Self {
            conversation_id: Some(conversation_id.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationEvent {
    Success,
    Failure,
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
        validate_aggregate_members(settings, &aggregate)?;
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
        validate_aggregate_members(settings, &self.aggregate)?;
        let members = member_pool_for_model(settings, &self.aggregate, model)?;
        self.refresh_pool_state(&members);
        let relay_id = match self.aggregate.strategy {
            AggregateRelayStrategy::Failover => member_id_at(&members, self.failover_index),
            AggregateRelayStrategy::ConversationRoundRobin => {
                self.select_for_conversation(context.conversation_id, &members)
            }
            AggregateRelayStrategy::RequestRoundRobin => self.select_next_request(&members),
            AggregateRelayStrategy::WeightedRoundRobin => self.select_next_weighted(&members),
        };
        relay_profile_by_id(settings, &relay_id).ok_or_else(|| SelectionError::UnknownMemberRelay {
            aggregate_id: self.aggregate.id.clone(),
            relay_id,
        })
    }

    pub fn peek(
        &self,
        settings: &BackendSettings,
        model: Option<&str>,
    ) -> Result<RelayProfile, SelectionError> {
        validate_aggregate_members(settings, &self.aggregate)?;
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
        relay_profile_by_id(settings, &relay_id).ok_or_else(|| SelectionError::UnknownMemberRelay {
            aggregate_id: self.aggregate.id.clone(),
            relay_id,
        })
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
    guard
        .as_mut()
        .expect("selector initialized")
        .select(settings, context, model)
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
    validate_aggregate_members(settings, &active_aggregate)?;
    let members = member_pool_for_model(settings, &active_aggregate, model)?;
    let start_index = members
        .iter()
        .position(|member| member.relay_id == relay_id)
        .map(|index| index + 1)
        .unwrap_or(0);
    (0..members.len().saturating_sub(1))
        .map(|offset| {
            let index = (start_index + offset) % members.len();
            &members[index]
        })
        .map(|member| {
            relay_profile_by_id(settings, &member.relay_id).ok_or_else(|| {
                SelectionError::UnknownMemberRelay {
                    aggregate_id: active_aggregate.id.clone(),
                    relay_id: member.relay_id.clone(),
                }
            })
        })
        .collect()
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

fn validate_aggregate_members(
    settings: &BackendSettings,
    aggregate: &AggregateRelayProfile,
) -> Result<(), SelectionError> {
    if aggregate.members.is_empty() {
        return Err(SelectionError::EmptyAggregateMembers {
            aggregate_id: aggregate.id.clone(),
        });
    }

    let relay_by_id = settings
        .relay_profiles
        .iter()
        .map(|profile| (profile.id.as_str(), profile))
        .collect::<HashMap<_, _>>();
    for member in &aggregate.members {
        let Some(relay) = relay_by_id.get(member.relay_id.as_str()) else {
            return Err(SelectionError::UnknownMemberRelay {
                aggregate_id: aggregate.id.clone(),
                relay_id: member.relay_id.clone(),
            });
        };
        if relay.base_url.trim().is_empty() || relay.api_key.trim().is_empty() {
            return Err(SelectionError::InvalidMemberRelay {
                aggregate_id: aggregate.id.clone(),
                relay_id: member.relay_id.clone(),
            });
        }
    }
    Ok(())
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

    let explicit_dispatch_members = aggregate_dispatch_member_pool(settings, aggregate, model);
    if !explicit_dispatch_members.is_empty() {
        return Ok(explicit_dispatch_members);
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
        .filter(|entry| entry.codex_model == model || entry.alias == model)
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
                    effective_model_mappings.insert(entry.alias, target_model);
                }
            }
        }

        relay.model_mappings = effective_model_mappings;
        relay.model_mappings_enabled = !relay.model_mappings.is_empty();

        relay
    })
}
