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

pub fn provider_display_name(profile: &RelayProfile) -> String {
    if profile.name.trim().is_empty() {
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
            alias: if looks_like_codex_model_key(&model) {
                codex_model_alias(&model, &provider_name, &model)
            } else {
                provider_label(&provider_name, &model)
            },
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

pub fn aggregate_catalog_aliases(
    aggregate: &AggregateRelayProfile,
    members: &[RelayProfile],
) -> Vec<AggregateModelAlias> {
    let dispatch_entries = aggregate_dispatch_entries(aggregate, members);
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
            alias: entry.alias,
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
                let provider_alias = codex_model_alias(&model, &provider_name, &model);
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
    use super::normalize_requested_model_name;

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
}
