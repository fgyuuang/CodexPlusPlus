use crate::settings::{
    AggregateRelayDispatchTarget, AggregateRelayModelMapping, AggregateRelayProfile, RelayProfile,
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateModelAlias {
    pub alias: String,
    pub target_model: String,
    pub via_mapping: bool,
    pub mapping_key: Option<String>,
    pub provider_id: String,
    pub provider_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateDispatchEntry {
    pub codex_model: String,
    pub provider_id: String,
    pub provider_name: String,
    pub target_model: String,
    pub alias: String,
    pub via_mapping: bool,
}

pub const TRUSTED_OFFICIAL_CODEX_MODELS: &[&str] = &[
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.3-codex",
];

pub const CLIPROXY_OFFICIAL_INTEGRATION_TYPE: &str = "cliproxy-official";
pub const CLIPROXY_OFFICIAL_PROVIDER_LABEL: &str = "CLIProxyAPI";
pub const CLIPROXY_GENERAL_INTEGRATION_TYPE: &str = "cliproxy";
pub const CLIPROXY_GENERAL_PROFILE_ID: &str = "managed-cliproxy";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectRelayAlias {
    pub alias: String,
    pub relay_id: String,
    pub target_model: String,
}

pub fn integration_is_excluded_from_aggregate(integration_type: &str) -> bool {
    integration_type
        .trim()
        .eq_ignore_ascii_case(CLIPROXY_OFFICIAL_INTEGRATION_TYPE)
        || integration_type
            .trim()
            .eq_ignore_ascii_case(CLIPROXY_GENERAL_INTEGRATION_TYPE)
}

pub fn integration_is_cliproxy_official(integration_type: &str) -> bool {
    integration_type
        .trim()
        .eq_ignore_ascii_case(CLIPROXY_OFFICIAL_INTEGRATION_TYPE)
}

pub fn integration_is_cliproxy_general(integration_type: &str) -> bool {
    integration_type
        .trim()
        .eq_ignore_ascii_case(CLIPROXY_GENERAL_INTEGRATION_TYPE)
}

pub fn cliproxy_official_api_aliases(profiles: &[RelayProfile]) -> Vec<DirectRelayAlias> {
    let official_profiles = profiles
        .iter()
        .filter(|profile| integration_is_cliproxy_official(&profile.integration_type))
        .collect::<Vec<_>>();
    let mut aliases = Vec::new();

    for official_model in TRUSTED_OFFICIAL_CODEX_MODELS {
        let mut candidates = official_profiles
            .iter()
            .flat_map(|profile| {
                relay_profile_model_ids(profile)
                    .into_iter()
                    .filter_map(move |target_model| {
                        (cliproxy_official_model_name(&target_model)
                            .is_some_and(|model| model.eq_ignore_ascii_case(official_model)))
                        .then(|| (profile.id.trim().to_string(), target_model))
                    })
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            let left_prefixed = left.1.contains('/');
            let right_prefixed = right.1.contains('/');
            left_prefixed
                .cmp(&right_prefixed)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.0.cmp(&right.0))
        });
        let Some((relay_id, target_model)) = candidates.into_iter().next() else {
            continue;
        };
        aliases.push(DirectRelayAlias {
            alias: provider_label(CLIPROXY_OFFICIAL_PROVIDER_LABEL, official_model),
            relay_id,
            target_model,
        });
    }
    aliases
}

pub fn cliproxy_general_api_aliases(
    profiles: &[RelayProfile],
    exclude_official_models: bool,
) -> Vec<DirectRelayAlias> {
    let mut aliases = Vec::new();
    let mut seen = HashSet::new();
    for profile in profiles.iter().filter(|profile| {
        integration_is_cliproxy_general(&profile.integration_type)
            || profile.id.trim() == CLIPROXY_GENERAL_PROFILE_ID
    }) {
        for target_model in relay_profile_model_ids(profile) {
            if exclude_official_models && cliproxy_official_model_name(&target_model).is_some() {
                continue;
            }
            let alias = provider_label(CLIPROXY_OFFICIAL_PROVIDER_LABEL, &target_model);
            if !seen.insert(alias.to_ascii_lowercase()) {
                continue;
            }
            aliases.push(DirectRelayAlias {
                alias,
                relay_id: profile.id.trim().to_string(),
                target_model,
            });
        }
    }
    aliases
}

pub fn cliproxy_direct_api_aliases(profiles: &[RelayProfile]) -> Vec<DirectRelayAlias> {
    let official_aliases = cliproxy_official_api_aliases(profiles);
    let mut aliases = official_aliases.clone();
    let mut seen = aliases
        .iter()
        .map(|alias| alias.alias.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for alias in cliproxy_general_api_aliases(profiles, !official_aliases.is_empty()) {
        if seen.insert(alias.alias.to_ascii_lowercase()) {
            aliases.push(alias);
        }
    }
    aliases
}

pub fn cliproxy_official_model_name(model: &str) -> Option<&str> {
    let model = model.trim();
    let base_model = model.rsplit('/').next().unwrap_or(model).trim();
    is_trusted_official_codex_model(base_model).then_some(base_model)
}

pub fn is_trusted_official_codex_model(model: &str) -> bool {
    let model = model.trim();
    TRUSTED_OFFICIAL_CODEX_MODELS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(model))
}

pub fn provider_display_name(profile: &RelayProfile) -> String {
    if profile
        .integration_type
        .trim()
        .eq_ignore_ascii_case(CLIPROXY_GENERAL_INTEGRATION_TYPE)
        || profile.id.trim() == CLIPROXY_GENERAL_PROFILE_ID
    {
        CLIPROXY_OFFICIAL_PROVIDER_LABEL.to_string()
    } else if profile.name.trim().is_empty() {
        profile.id.trim().to_string()
    } else {
        profile.name.trim().to_string()
    }
}

pub fn provider_label(provider_name: &str, model: &str) -> String {
    format!("{}:{}", provider_name.trim(), model.trim())
}

pub fn codex_model_alias(codex_model: &str, provider_name: &str, target_model: &str) -> String {
    let codex_model = codex_model.trim();
    let target_model = target_model.trim();
    let provider = provider_name.trim();
    if codex_model.eq_ignore_ascii_case(target_model) {
        format!("{}({})", codex_model, provider)
    } else {
        format!("{}({}:{})", codex_model, provider, target_model)
    }
}

pub fn relay_profile_model_ids(profile: &RelayProfile) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    profile
        .model_list
        .split(['\r', '\n', ','])
        .chain(std::iter::once(profile.model.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert((*value).to_string()))
        .map(ToString::to_string)
        .collect()
}

pub fn aggregate_model_aliases_for_member(
    profile: &RelayProfile,
    include_mapping_aliases: bool,
) -> Vec<AggregateModelAlias> {
    let provider_name = provider_display_name(profile);
    let provider_id = profile.id.trim().to_string();
    let mut aliases = relay_profile_model_ids(profile)
        .into_iter()
        .map(|model| AggregateModelAlias {
            alias: provider_label(&provider_name, &model),
            target_model: model,
            via_mapping: false,
            mapping_key: None,
            provider_id: provider_id.clone(),
            provider_name: provider_name.clone(),
        })
        .collect::<Vec<_>>();

    if include_mapping_aliases {
        let mut mapping_aliases = profile
            .model_mappings
            .iter()
            .filter_map(|(mapping_key, mapped_model)| {
                let mapping_key = mapping_key.trim();
                let mapped_model = mapped_model.trim();
                if mapping_key.is_empty() || mapped_model.is_empty() {
                    return None;
                }
                Some(AggregateModelAlias {
                    alias: codex_model_alias(mapping_key, &provider_name, mapped_model),
                    target_model: mapped_model.to_string(),
                    via_mapping: true,
                    mapping_key: Some(mapping_key.to_string()),
                    provider_id: provider_id.clone(),
                    provider_name: provider_name.clone(),
                })
            })
            .collect::<Vec<_>>();
        mapping_aliases.sort_by(|left, right| left.alias.cmp(&right.alias));
        aliases.extend(mapping_aliases);
    }

    aliases
}

pub fn aggregate_mapping_entries_for_member(profile: &RelayProfile) -> Vec<AggregateDispatchEntry> {
    if !profile.model_mappings_enabled {
        return Vec::new();
    }

    let provider_name = provider_display_name(profile);
    let provider_id = profile.id.trim().to_string();
    let mut entries = profile
        .model_mappings
        .iter()
        .filter_map(|(codex_model, target_model)| {
            let codex_model = codex_model.trim();
            let target_model = target_model.trim();
            if codex_model.is_empty() || target_model.is_empty() {
                return None;
            }
            Some(AggregateDispatchEntry {
                codex_model: codex_model.to_string(),
                provider_id: provider_id.clone(),
                provider_name: provider_name.clone(),
                target_model: target_model.to_string(),
                alias: codex_model_alias(codex_model, &provider_name, target_model),
                via_mapping: true,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.alias.cmp(&right.alias));
    entries
}

pub fn aggregate_dispatch_entries(
    aggregate: &AggregateRelayProfile,
    members: &[RelayProfile],
) -> Vec<AggregateDispatchEntry> {
    let relay_by_id = members
        .iter()
        .map(|member| (member.id.as_str(), member))
        .collect::<HashMap<_, _>>();
    let explicit_mapping_keys = aggregate
        .model_mappings
        .iter()
        .map(|mapping| mapping.codex_model.trim())
        .filter(|mapping| !mapping.is_empty())
        .map(ToString::to_string)
        .collect::<HashSet<_>>();
    let mut entries = Vec::new();

    for mapping in &aggregate.model_mappings {
        entries.extend(explicit_dispatch_entries_for_mapping(mapping, &relay_by_id));
    }

    if aggregate.model_mappings_enabled {
        for member_ref in &aggregate.members {
            let Some(member) = relay_by_id.get(member_ref.relay_id.as_str()) else {
                continue;
            };
            entries.extend(implicit_dispatch_entries_for_member(
                member,
                &explicit_mapping_keys,
            ));
        }
    }

    entries
}

pub fn aggregate_replacement_model_aliases(
    aggregate: &AggregateRelayProfile,
    members: &[RelayProfile],
) -> Vec<String> {
    let mut entries = aggregate_dispatch_entries(aggregate, members);
    order_catalog_dispatch_entries_by_member(&mut entries, aggregate);
    let mut model_order = Vec::new();
    let mut labels_by_model = HashMap::<String, Vec<String>>::new();
    for entry in entries {
        let codex_model = entry.codex_model.trim().to_string();
        if codex_model.is_empty() || !looks_like_codex_model_key(&codex_model) {
            continue;
        }
        if !labels_by_model.contains_key(&codex_model) {
            model_order.push(codex_model.clone());
        }
        let label = if codex_model.eq_ignore_ascii_case(entry.target_model.trim()) {
            entry.provider_name.trim().to_string()
        } else {
            format!(
                "{}:{}",
                entry.provider_name.trim(),
                entry.target_model.trim()
            )
        };
        let labels = labels_by_model.entry(codex_model).or_default();
        if !label.is_empty() && !labels.iter().any(|existing| existing == &label) {
            labels.push(label);
        }
    }
    model_order.sort_by(compare_catalog_model_names);

    model_order
        .into_iter()
        .filter_map(|model| {
            let labels = labels_by_model.remove(&model)?;
            (!labels.is_empty()).then(|| format!("{}({})", model, labels.join("|")))
        })
        .collect()
}

pub fn aggregate_catalog_aliases(
    aggregate: &AggregateRelayProfile,
    members: &[RelayProfile],
) -> Vec<AggregateModelAlias> {
    let mut dispatch_entries = aggregate_dispatch_entries(aggregate, members);
    order_catalog_dispatch_entries_by_member(&mut dispatch_entries, aggregate);
    let mut aliases = Vec::new();
    for entry in dispatch_entries {
        aliases.push(AggregateModelAlias {
            alias: entry.codex_model.clone(),
            target_model: entry.target_model.clone(),
            via_mapping: entry.via_mapping,
            mapping_key: Some(entry.codex_model.clone()),
            provider_id: entry.provider_id.clone(),
            provider_name: entry.provider_name.clone(),
        });
        aliases.push(AggregateModelAlias {
            alias: provider_label(&entry.provider_name, &entry.target_model),
            target_model: entry.target_model,
            via_mapping: entry.via_mapping,
            mapping_key: Some(entry.codex_model),
            provider_id: entry.provider_id,
            provider_name: entry.provider_name,
        });
    }
    let dispatch_alias_count = aliases.len();
    let mut seen_details = aliases
        .iter()
        .map(|alias| {
            (
                alias.alias.clone(),
                alias.provider_id.clone(),
                alias.target_model.clone(),
                alias.via_mapping,
            )
        })
        .collect::<HashSet<_>>();

    let relay_by_id = members
        .iter()
        .map(|member| (member.id.as_str(), member))
        .collect::<HashMap<_, _>>();
    let mut repeated_direct_models = HashMap::<String, Vec<(String, String)>>::new();
    for member_ref in &aggregate.members {
        let Some(member) = relay_by_id.get(member_ref.relay_id.as_str()) else {
            continue;
        };
        let provider_name = provider_display_name(member);
        let provider_id = member.id.trim().to_string();
        for model in relay_profile_model_ids(member) {
            repeated_direct_models
                .entry(model.clone())
                .or_default()
                .push((provider_id.clone(), provider_name.clone()));
            if looks_like_codex_model_key(&model) {
                let detail_key = (model.clone(), provider_id.clone(), model.clone(), false);
                if seen_details.insert(detail_key) {
                    aliases.push(AggregateModelAlias {
                        alias: model.clone(),
                        target_model: model.clone(),
                        via_mapping: false,
                        mapping_key: None,
                        provider_id: provider_id.clone(),
                        provider_name: provider_name.clone(),
                    });
                }
                let provider_alias = provider_label(&provider_name, &model);
                let provider_detail_key = (
                    provider_alias.clone(),
                    provider_id.clone(),
                    model.clone(),
                    false,
                );
                if seen_details.insert(provider_detail_key) {
                    aliases.push(AggregateModelAlias {
                        alias: provider_alias,
                        target_model: model,
                        via_mapping: false,
                        mapping_key: None,
                        provider_id: provider_id.clone(),
                        provider_name: provider_name.clone(),
                    });
                }
                continue;
            }
            aliases.push(AggregateModelAlias {
                alias: provider_label(&provider_name, &model),
                target_model: model,
                via_mapping: false,
                mapping_key: None,
                provider_id: provider_id.clone(),
                provider_name: provider_name.clone(),
            });
        }
    }

    for (model, providers) in repeated_direct_models {
        if looks_like_codex_model_key(&model) || providers.len() <= 1 {
            continue;
        }
        for (provider_id, provider_name) in providers {
            let detail_key = (model.clone(), provider_id.clone(), model.clone(), false);
            if seen_details.insert(detail_key) {
                aliases.push(AggregateModelAlias {
                    alias: model.clone(),
                    target_model: model.clone(),
                    via_mapping: false,
                    mapping_key: None,
                    provider_id,
                    provider_name,
                });
            }
        }
    }

    let split_index = dispatch_alias_count.min(aliases.len());
    let (head, tail) = aliases.split_at_mut(split_index);
    tail.sort_by(|left, right| {
        let left_rank = aggregate_catalog_tail_rank(left);
        let right_rank = aggregate_catalog_tail_rank(right);
        left_rank
            .cmp(&right_rank)
            .then(left.alias.cmp(&right.alias))
            .then(left.provider_id.cmp(&right.provider_id))
    });
    let _ = head;
    aliases
}

/// Returns the user-facing aggregate model order.
///
/// Bare Codex model keys are kept first because they are the stable models
/// shown by Codex (their provider replacement is rendered through metadata).
/// Provider-qualified models follow in aggregate member order and retain the
/// model order configured on each member. This prevents a member's first raw
/// model (for example `composer-2.5`) from becoming the aggregate default.
pub fn aggregate_catalog_model_list(
    aggregate: &AggregateRelayProfile,
    members: &[RelayProfile],
    aliases: &[AggregateModelAlias],
) -> Vec<String> {
    let mut models = Vec::new();
    let mut seen = HashSet::new();

    let mut official_models = aliases
        .iter()
        .filter(|alias| {
            !alias.alias.contains(':')
                && !alias.alias.contains('(')
                && looks_like_codex_model_key(&alias.alias)
        })
        .map(|alias| alias.alias.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    official_models.sort_by(compare_catalog_model_names);
    for model in official_models {
        if seen.insert(model.clone()) {
            models.push(model);
        }
    }

    let alias_keys = aliases
        .iter()
        .filter(|alias| alias.alias.contains(':') && !alias.alias.contains('('))
        .map(|alias| {
            (
                alias.provider_id.as_str(),
                alias.alias.as_str(),
                alias.target_model.as_str(),
            )
        })
        .collect::<Vec<_>>();
    let members_by_id = members
        .iter()
        .map(|member| (member.id.as_str(), member))
        .collect::<HashMap<_, _>>();

    for member_ref in &aggregate.members {
        let Some(member) = members_by_id.get(member_ref.relay_id.as_str()) else {
            continue;
        };
        let provider = provider_display_name(member);
        for raw_model in relay_profile_model_ids(member) {
            let label = provider_label(&provider, &raw_model);
            if alias_keys.iter().any(|(provider_id, alias, target)| {
                *provider_id == member.id.as_str()
                    && *alias == label.as_str()
                    && *target == raw_model.as_str()
            }) && seen.insert(label.clone())
            {
                models.push(label);
            }
        }

        // Explicit mappings may target a model not present in the member's
        // modelList. Append those labels after the member's configured order.
        for alias in aliases.iter().filter(|alias| {
            alias.provider_id == member.id
                && alias.alias.contains(':')
                && !alias.alias.contains('(')
        }) {
            if seen.insert(alias.alias.clone()) {
                models.push(alias.alias.clone());
            }
        }
    }

    // Keep malformed/orphaned entries visible instead of dropping them, but
    // place them after known aggregate members.
    for alias in aliases
        .iter()
        .filter(|alias| alias.alias.contains(':') && !alias.alias.contains('('))
    {
        if seen.insert(alias.alias.clone()) {
            models.push(alias.alias.clone());
        }
    }

    models
}

fn compare_catalog_model_names(left: &String, right: &String) -> std::cmp::Ordering {
    fn rank(model: &str) -> (usize, String) {
        const PREFERRED: [&str; 7] = [
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.3-codex",
        ];
        let normalized = model.trim().to_ascii_lowercase();
        let preferred_rank = PREFERRED
            .iter()
            .position(|candidate| *candidate == normalized)
            .unwrap_or(usize::MAX);
        let family_rank = if normalized.starts_with("gpt-") {
            1
        } else if normalized.starts_with("codex-") {
            2
        } else {
            3
        };
        (
            if preferred_rank == usize::MAX {
                100 + family_rank
            } else {
                preferred_rank
            },
            normalized,
        )
    }

    rank(left).cmp(&rank(right))
}

fn aggregate_catalog_tail_rank(alias: &AggregateModelAlias) -> u8 {
    let text = alias.alias.trim();
    if looks_like_codex_model_key(text) {
        return 0;
    }
    if !text.contains(':') && !text.contains('(') {
        return 1;
    }
    2
}

// Model selection still follows the target order configured on a mapping. The
// catalog is display-only, so keep its provider-specific entries aligned with
// the aggregate member order instead.
fn order_catalog_dispatch_entries_by_member(
    entries: &mut [AggregateDispatchEntry],
    aggregate: &AggregateRelayProfile,
) {
    let member_order = aggregate
        .members
        .iter()
        .enumerate()
        .map(|(index, member)| (member.relay_id.as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut start = 0;
    while start < entries.len() {
        let codex_model = entries[start].codex_model.clone();
        let mut end = start + 1;
        while end < entries.len() && entries[end].codex_model == codex_model {
            end += 1;
        }
        entries[start..end].sort_by_key(|entry| {
            member_order
                .get(entry.provider_id.as_str())
                .copied()
                .unwrap_or(usize::MAX)
        });
        start = end;
    }
}

fn explicit_dispatch_entries_for_mapping(
    mapping: &AggregateRelayModelMapping,
    relay_by_id: &HashMap<&str, &RelayProfile>,
) -> Vec<AggregateDispatchEntry> {
    let codex_model = mapping.codex_model.trim();
    if codex_model.is_empty() {
        return Vec::new();
    }

    mapping
        .targets
        .iter()
        .filter_map(|target| dispatch_entry_from_target(codex_model, target, relay_by_id))
        .collect()
}

fn dispatch_entry_from_target(
    codex_model: &str,
    target: &AggregateRelayDispatchTarget,
    relay_by_id: &HashMap<&str, &RelayProfile>,
) -> Option<AggregateDispatchEntry> {
    let relay_id = target.relay_id.trim();
    let target_model = target.target_model.trim();
    if relay_id.is_empty() || target_model.is_empty() {
        return None;
    }
    let member = relay_by_id.get(relay_id)?;
    let provider_name = provider_display_name(member);
    Some(AggregateDispatchEntry {
        codex_model: codex_model.to_string(),
        provider_id: relay_id.to_string(),
        provider_name: provider_name.clone(),
        target_model: target_model.to_string(),
        alias: codex_model_alias(codex_model, &provider_name, target_model),
        via_mapping: true,
    })
}

fn implicit_dispatch_entries_for_member(
    member: &RelayProfile,
    explicit_mapping_keys: &HashSet<String>,
) -> Vec<AggregateDispatchEntry> {
    let provider_name = provider_display_name(member);
    let provider_id = member.id.trim().to_string();
    let relay_models = relay_profile_model_ids(member);

    relay_models
        .into_iter()
        .filter(|model| looks_like_codex_model_key(model))
        .filter(|model| !explicit_mapping_keys.contains(model))
        .map(|model| AggregateDispatchEntry {
            codex_model: model.clone(),
            provider_id: provider_id.clone(),
            provider_name: provider_name.clone(),
            target_model: model.clone(),
            alias: codex_model_alias(&model, &provider_name, &model),
            via_mapping: false,
        })
        .collect()
}

pub fn looks_like_codex_model_key(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    normalized.starts_with("gpt-") || normalized.starts_with("codex-")
}

pub fn normalize_requested_model_name(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let Some(open_index) = trimmed.find('(') else {
        return trimmed.to_string();
    };
    if !trimmed.ends_with(')') {
        return trimmed.to_string();
    }
    let base = trimmed[..open_index].trim();
    if base.is_empty() {
        return trimmed.to_string();
    }
    let suffix = trimmed[open_index + 1..trimmed.len() - 1].trim();
    if suffix.is_empty() || !looks_like_codex_model_key(base) {
        return trimmed.to_string();
    }
    base.to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        cliproxy_direct_api_aliases, cliproxy_general_api_aliases,
        integration_is_cliproxy_official, integration_is_excluded_from_aggregate,
        normalize_requested_model_name, provider_display_name,
    };
    use crate::settings::RelayProfile;

    #[test]
    fn normalizes_aggregate_codex_display_labels_with_or_without_target_model() {
        assert_eq!(
            normalize_requested_model_name("gpt-5.4(供应商一|供应商二:vendor-gpt-5.4)"),
            "gpt-5.4"
        );
        assert_eq!(
            normalize_requested_model_name("gpt-5.4(供应商一|供应商二)"),
            "gpt-5.4"
        );
        assert_eq!(
            normalize_requested_model_name("custom-model(供应商一)"),
            "custom-model(供应商一)"
        );
    }

    #[test]
    fn managed_cliproxy_general_profile_uses_stable_provider_label() {
        let profile = RelayProfile {
            id: "managed-cliproxy".to_string(),
            name: "CLIProxyAPI 通用中转".to_string(),
            integration_type: "cliproxy".to_string(),
            ..RelayProfile::default()
        };

        assert_eq!(provider_display_name(&profile), "CLIProxyAPI");
    }

    #[test]
    fn managed_cliproxy_profiles_are_excluded_from_aggregate_but_only_official_is_direct() {
        assert!(integration_is_excluded_from_aggregate("cliproxy"));
        assert!(integration_is_excluded_from_aggregate("cliproxy-official"));
        assert!(!integration_is_cliproxy_official("cliproxy"));
        assert!(integration_is_cliproxy_official("cliproxy-official"));
    }

    #[test]
    fn cliproxy_general_aliases_keep_non_official_models_and_yield_to_special_official_channel() {
        let profiles = vec![
            RelayProfile {
                id: "managed-cliproxy".to_string(),
                integration_type: "cliproxy".to_string(),
                model: "gemini-2.5-pro".to_string(),
                model_list: "gpt-5.6-sol\ngemini-2.5-pro".to_string(),
                ..RelayProfile::default()
            },
            RelayProfile {
                id: "managed-cliproxy-official".to_string(),
                integration_type: "cliproxy-official".to_string(),
                model: "account-2/gpt-5.6-sol".to_string(),
                model_list: "account-2/gpt-5.6-sol".to_string(),
                ..RelayProfile::default()
            },
        ];

        assert_eq!(
            cliproxy_general_api_aliases(&profiles, true)
                .into_iter()
                .map(|alias| alias.alias)
                .collect::<Vec<_>>(),
            ["CLIProxyAPI:gemini-2.5-pro"]
        );
        assert_eq!(
            cliproxy_general_api_aliases(&profiles[..1], false)
                .into_iter()
                .map(|alias| alias.alias)
                .collect::<Vec<_>>(),
            ["CLIProxyAPI:gpt-5.6-sol", "CLIProxyAPI:gemini-2.5-pro"]
        );
        let aliases = cliproxy_direct_api_aliases(&profiles);
        assert_eq!(
            aliases
                .iter()
                .map(|alias| alias.alias.as_str())
                .collect::<Vec<_>>(),
            ["CLIProxyAPI:gpt-5.6-sol", "CLIProxyAPI:gemini-2.5-pro"]
        );
        assert_eq!(aliases[0].relay_id, "managed-cliproxy-official");
        assert_eq!(aliases[1].relay_id, "managed-cliproxy");
    }
}
