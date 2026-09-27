use std::collections::HashSet;

use serde_json::{Value, json};

use crate::aggregate_model_alias::{
    self, CLIPROXY_GENERAL_PROFILE_ID, CLIPROXY_OFFICIAL_INTEGRATION_TYPE,
};
use crate::settings::{
    BackendSettings, ModelCapabilityBinding, ModelRouteDescriptor, OfficialExperienceSourceKind,
    RelayMode,
};

pub const PRIMARY_OFFICIAL_PROVIDER_ID: &str = "openai";
pub const DIRECT_OFFICIAL_PROVIDER_PREFIX: &str = "codex_plus_official_";
pub const CLI_OFFICIAL_PROVIDER_ID: &str = "codex_plus_cli_official";
pub const CLI_GENERAL_PROVIDER_ID: &str = "codex_plus_cli_general";
pub const LEGACY_CUSTOM_PROVIDER_ID: &str = "custom";

pub fn stable_provider_component(value: &str) -> String {
    let mut output = String::new();
    for byte in value.trim().bytes() {
        match byte {
            b'a'..=b'z' | b'0'..=b'9' => output.push(char::from(byte)),
            _ => {
                use std::fmt::Write;
                let _ = write!(output, "_{byte:02x}_");
            }
        }
    }
    output
}

pub fn relay_provider_id(source_id: &str) -> String {
    let component = stable_provider_component(source_id);
    format!(
        "codex_plus_relay_{}",
        if component.is_empty() {
            "source"
        } else {
            &component
        }
    )
}

pub fn aggregate_provider_id(source_id: &str) -> String {
    let component = stable_provider_component(source_id);
    format!(
        "codex_plus_aggregate_{}",
        if component.is_empty() {
            "source"
        } else {
            &component
        }
    )
}

pub fn direct_official_provider_id(account_id: &str) -> String {
    format!(
        "{DIRECT_OFFICIAL_PROVIDER_PREFIX}{}",
        stable_provider_component(account_id)
    )
}

pub fn direct_official_routing_slug(account_id: &str, model: &str) -> String {
    format!("{}:{model}", direct_official_provider_id(account_id))
}

fn direct_official_descriptors_for_account(
    account_id: &str,
    label: &str,
    entries: &[Value],
    trusted: &HashSet<String>,
) -> Vec<ModelRouteDescriptor> {
    entries
        .iter()
        .filter(|entry| entry.get("supported_in_api").and_then(Value::as_bool) != Some(false))
        .filter(|entry| {
            entry
                .get("visibility")
                .and_then(Value::as_str)
                .is_none_or(|visibility| visibility.eq_ignore_ascii_case("list"))
        })
        .filter_map(|entry| entry.get("slug").and_then(Value::as_str))
        .filter(|slug| trusted.contains(&slug.to_ascii_lowercase()))
        .map(|slug| ModelRouteDescriptor {
            routing_slug: direct_official_routing_slug(account_id, slug),
            display_name: format!("{slug}({label})"),
            provider_id: direct_official_provider_id(account_id),
            source_kind: OfficialExperienceSourceKind::DirectOfficial,
            source_id: account_id.to_string(),
            upstream_model: slug.to_string(),
            capability_slug: slug.to_string(),
            failover_policy: "strict".to_string(),
        })
        .collect()
}

pub fn provider_id_for_source(kind: OfficialExperienceSourceKind, source_id: &str) -> String {
    match kind {
        OfficialExperienceSourceKind::MainOfficial => PRIMARY_OFFICIAL_PROVIDER_ID.to_string(),
        OfficialExperienceSourceKind::DirectOfficial => direct_official_provider_id(source_id),
        OfficialExperienceSourceKind::CliOfficial => CLI_OFFICIAL_PROVIDER_ID.to_string(),
        OfficialExperienceSourceKind::CliGeneral => CLI_GENERAL_PROVIDER_ID.to_string(),
        OfficialExperienceSourceKind::Relay => relay_provider_id(source_id),
        OfficialExperienceSourceKind::Aggregate => aggregate_provider_id(source_id),
        OfficialExperienceSourceKind::LegacyCustom => LEGACY_CUSTOM_PROVIDER_ID.to_string(),
    }
}

pub fn local_route_base_url(port: u16, provider_id: &str) -> String {
    format!("http://127.0.0.1:{port}/v1/routes/{provider_id}")
}

pub fn route_provider_id_from_path(path: &str) -> Option<String> {
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    let normalized = path.strip_prefix('/').unwrap_or(path);
    let mut parts = normalized.split('/');
    if parts.next()? != "v1" || parts.next()? != "routes" {
        return None;
    }
    let provider_id = parts.next()?.trim();
    if provider_id.is_empty() {
        return None;
    }
    Some(provider_id.to_string())
}

pub fn route_operation_path(path: &str) -> &str {
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    let normalized = path.strip_prefix('/').unwrap_or(path);
    let mut parts = normalized.split('/');
    if parts.next() != Some("v1") || parts.next() != Some("routes") || parts.next().is_none() {
        return path;
    }
    let remainder = parts.collect::<Vec<_>>().join("/");
    match remainder.as_str() {
        "responses" => "/v1/responses",
        "responses/compact" => "/v1/responses/compact",
        "images/generations" => "/v1/images/generations",
        "images/edits" => "/v1/images/edits",
        _ => path,
    }
}

fn source_label(settings: &BackendSettings, binding: &ModelCapabilityBinding) -> String {
    match binding.source_kind {
        OfficialExperienceSourceKind::MainOfficial => "主官方账号".to_string(),
        OfficialExperienceSourceKind::DirectOfficial => "直连官方账号".to_string(),
        OfficialExperienceSourceKind::CliOfficial => settings
            .official_experience
            .cli_official_label
            .trim()
            .to_string(),
        OfficialExperienceSourceKind::CliGeneral => "CLIProxyAPI".to_string(),
        OfficialExperienceSourceKind::Relay => settings
            .relay_profiles
            .iter()
            .find(|profile| profile.id == binding.source_id)
            .map(|profile| profile.name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or(binding.source_id.as_str())
            .to_string(),
        OfficialExperienceSourceKind::Aggregate => settings
            .aggregate_relay_profiles
            .iter()
            .find(|profile| profile.id == binding.source_id)
            .map(|profile| profile.name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or(binding.source_id.as_str())
            .to_string(),
        OfficialExperienceSourceKind::LegacyCustom => "旧 custom 会话".to_string(),
    }
}

fn descriptor_from_binding(
    settings: &BackendSettings,
    binding: &ModelCapabilityBinding,
) -> ModelRouteDescriptor {
    let label = source_label(settings, binding);
    let display_name = match binding.source_kind {
        OfficialExperienceSourceKind::MainOfficial => binding.capability_slug.clone(),
        OfficialExperienceSourceKind::DirectOfficial => binding.routing_slug.clone(),
        OfficialExperienceSourceKind::CliOfficial => binding.routing_slug.clone(),
        OfficialExperienceSourceKind::Relay => {
            format!("{}:{}", label, binding.upstream_model)
        }
        OfficialExperienceSourceKind::CliGeneral
        | OfficialExperienceSourceKind::Aggregate
        | OfficialExperienceSourceKind::LegacyCustom => binding.routing_slug.clone(),
    };
    ModelRouteDescriptor {
        routing_slug: binding.routing_slug.clone(),
        display_name,
        provider_id: provider_id_for_source(binding.source_kind, &binding.source_id),
        source_kind: binding.source_kind,
        source_id: binding.source_id.clone(),
        upstream_model: binding.upstream_model.clone(),
        capability_slug: binding.capability_slug.clone(),
        failover_policy: "strict".to_string(),
    }
}

pub fn model_route_descriptors(settings: &BackendSettings) -> Vec<ModelRouteDescriptor> {
    if !settings.official_experience.enabled {
        return Vec::new();
    }
    let trusted =
        crate::official_model_catalog::trusted_official_model_slugs_for_settings(settings);
    let trusted_lower = trusted
        .iter()
        .map(|slug| slug.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut descriptors = Vec::new();
    let mut seen = HashSet::new();

    for slug in crate::official_model_catalog::visible_official_model_slugs_for_settings(settings) {
        let descriptor = ModelRouteDescriptor {
            routing_slug: slug.clone(),
            display_name: slug.clone(),
            provider_id: PRIMARY_OFFICIAL_PROVIDER_ID.to_string(),
            source_kind: OfficialExperienceSourceKind::MainOfficial,
            source_id: settings
                .official_experience
                .primary_official_account_id
                .clone(),
            upstream_model: slug.clone(),
            capability_slug: slug.clone(),
            failover_policy: "strict".to_string(),
        };
        seen.insert(slug.to_ascii_lowercase());
        descriptors.push(descriptor);
    }

    let primary_id = if settings.active_official_account_id.trim().is_empty() {
        settings
            .official_experience
            .primary_official_account_id
            .trim()
    } else {
        settings.active_official_account_id.trim()
    };
    if let Ok(accounts) = crate::official_accounts::OfficialAccountStore::default().list() {
        for account in accounts.into_iter().filter(|account| {
            account.enabled && account.status == "ready" && account.id != primary_id
        }) {
            let Some(entries) =
                crate::official_model_catalog::cached_official_model_entries_for_account(
                    &account.id,
                )
            else {
                continue;
            };
            let label = if account.name.trim().is_empty() {
                "官方账号 2"
            } else {
                account.name.trim()
            };
            for route in direct_official_descriptors_for_account(
                &account.id,
                label,
                &entries,
                &trusted_lower,
            ) {
                if seen.insert(route.routing_slug.to_ascii_lowercase()) {
                    descriptors.push(route);
                }
            }
        }
    }

    if settings.official_experience.cli_official_enabled {
        for alias in aggregate_model_alias::cliproxy_official_api_aliases_for_settings(
            &settings.relay_profiles,
            settings,
        ) {
            let capability_slug = aggregate_model_alias::cliproxy_official_model_name_for_settings(
                &alias.target_model,
                settings,
            )
            .unwrap_or(alias.target_model.trim())
            .to_string();
            if !trusted_lower.contains(&capability_slug.to_ascii_lowercase())
                || !seen.insert(alias.alias.to_ascii_lowercase())
            {
                continue;
            }
            descriptors.push(ModelRouteDescriptor {
                display_name: alias.alias.clone(),
                routing_slug: alias.alias,
                provider_id: CLI_OFFICIAL_PROVIDER_ID.to_string(),
                source_kind: OfficialExperienceSourceKind::CliOfficial,
                source_id: alias.relay_id,
                upstream_model: alias.target_model,
                capability_slug,
                failover_policy: "strict".to_string(),
            });
        }
    }

    let active_aggregate = settings.active_aggregate_relay_profile();
    let active_relay = if active_aggregate.is_none() {
        settings.relay_profiles.iter().find(|profile| {
            profile.id == settings.active_relay_id
                && matches!(profile.relay_mode, RelayMode::MixedApi | RelayMode::PureApi)
        })
    } else {
        None
    };
    let mut valid_bindings = HashSet::new();
    for profile in &settings.relay_profiles {
        if profile.relay_mode == RelayMode::Official || profile.relay_mode == RelayMode::Aggregate {
            continue;
        }
        let source_kind =
            if aggregate_model_alias::integration_is_cliproxy_general(&profile.integration_type)
                || profile.id == CLIPROXY_GENERAL_PROFILE_ID
            {
                if !settings.official_experience.cli_general_enabled {
                    continue;
                }
                OfficialExperienceSourceKind::CliGeneral
            } else if aggregate_model_alias::integration_is_cliproxy_official(
                &profile.integration_type,
            ) {
                continue;
            } else {
                if active_relay.is_none_or(|active| active.id != profile.id) {
                    continue;
                }
                OfficialExperienceSourceKind::Relay
            };
        for upstream_model in aggregate_model_alias::relay_profile_model_ids(profile) {
            let routing_slug = match source_kind {
                OfficialExperienceSourceKind::CliGeneral => {
                    format!("CLIProxyAPI:{upstream_model}")
                }
                _ => format!("{}:{upstream_model}", profile.id),
            };
            valid_bindings.insert((
                source_kind,
                profile.id.to_ascii_lowercase(),
                routing_slug.to_ascii_lowercase(),
                upstream_model.to_ascii_lowercase(),
            ));
            let Some(capability_slug) = trusted
                .iter()
                .find(|slug| slug.eq_ignore_ascii_case(&upstream_model))
            else {
                continue;
            };
            if !seen.insert(routing_slug.to_ascii_lowercase()) {
                continue;
            }
            descriptors.push(descriptor_from_binding(
                settings,
                &ModelCapabilityBinding {
                    routing_slug,
                    capability_slug: capability_slug.clone(),
                    source_kind,
                    source_id: profile.id.clone(),
                    upstream_model,
                },
            ));
        }
    }

    for aggregate in active_aggregate.iter() {
        let members = aggregate
            .members
            .iter()
            .filter_map(|member| {
                settings
                    .relay_profiles
                    .iter()
                    .find(|profile| profile.id == member.relay_id)
                    .cloned()
            })
            .collect::<Vec<_>>();
        let mut aggregate_slugs =
            aggregate_model_alias::aggregate_replacement_model_aliases(aggregate, &members);
        aggregate_slugs.extend(
            aggregate_model_alias::aggregate_catalog_aliases(aggregate, &members)
                .into_iter()
                .filter(|alias| alias.alias.contains(':') && !alias.alias.contains('('))
                .map(|alias| alias.alias),
        );
        for routing_slug in aggregate_slugs {
            valid_bindings.insert((
                OfficialExperienceSourceKind::Aggregate,
                aggregate.id.to_ascii_lowercase(),
                routing_slug.to_ascii_lowercase(),
                routing_slug.to_ascii_lowercase(),
            ));
            let Some((base_slug, _)) = routing_slug.split_once('(') else {
                continue;
            };
            let Some(capability_slug) = trusted
                .iter()
                .find(|slug| slug.eq_ignore_ascii_case(base_slug.trim()))
            else {
                continue;
            };
            if !seen.insert(routing_slug.to_ascii_lowercase()) {
                continue;
            }
            descriptors.push(descriptor_from_binding(
                settings,
                &ModelCapabilityBinding {
                    routing_slug: routing_slug.clone(),
                    capability_slug: capability_slug.clone(),
                    source_kind: OfficialExperienceSourceKind::Aggregate,
                    source_id: aggregate.id.clone(),
                    upstream_model: routing_slug,
                },
            ));
        }
    }

    for binding in &settings.official_experience.capability_bindings {
        if !trusted_lower.contains(&binding.capability_slug.to_ascii_lowercase())
            || !valid_bindings.contains(&(
                binding.source_kind,
                binding.source_id.to_ascii_lowercase(),
                binding.routing_slug.to_ascii_lowercase(),
                binding.upstream_model.to_ascii_lowercase(),
            ))
            || seen.contains(&binding.routing_slug.to_ascii_lowercase())
        {
            continue;
        }
        if binding.source_kind != OfficialExperienceSourceKind::MainOfficial
            && trusted_lower.contains(&binding.routing_slug.to_ascii_lowercase())
        {
            continue;
        }
        if matches!(
            binding.source_kind,
            OfficialExperienceSourceKind::MainOfficial
                | OfficialExperienceSourceKind::DirectOfficial
                | OfficialExperienceSourceKind::LegacyCustom
        ) {
            continue;
        }
        if binding.source_kind == OfficialExperienceSourceKind::CliOfficial
            && !settings.official_experience.cli_official_enabled
        {
            continue;
        }
        if binding.source_kind == OfficialExperienceSourceKind::CliGeneral
            && !settings.official_experience.cli_general_enabled
        {
            continue;
        }
        seen.insert(binding.routing_slug.to_ascii_lowercase());
        descriptors.push(descriptor_from_binding(settings, binding));
    }

    descriptors.sort_by_key(|descriptor| match descriptor.source_kind {
        OfficialExperienceSourceKind::MainOfficial
            if descriptor
                .routing_slug
                .to_ascii_lowercase()
                .starts_with("gpt-") =>
        {
            0
        }
        OfficialExperienceSourceKind::MainOfficial => 1,
        OfficialExperienceSourceKind::DirectOfficial => 2,
        OfficialExperienceSourceKind::CliOfficial
            if descriptor
                .capability_slug
                .to_ascii_lowercase()
                .starts_with("gpt-") =>
        {
            3
        }
        OfficialExperienceSourceKind::CliGeneral
            if descriptor
                .upstream_model
                .to_ascii_lowercase()
                .starts_with("gpt-") =>
        {
            4
        }
        OfficialExperienceSourceKind::Relay | OfficialExperienceSourceKind::Aggregate => 5,
        OfficialExperienceSourceKind::CliOfficial | OfficialExperienceSourceKind::CliGeneral => 6,
        OfficialExperienceSourceKind::LegacyCustom => 7,
    });
    descriptors
}

pub fn extension_model_priority_start(settings: &BackendSettings) -> i64 {
    crate::official_model_catalog::official_model_entries_for_settings(settings)
        .iter()
        .filter_map(|entry| entry.get("priority").and_then(Value::as_i64))
        .max()
        .unwrap_or(0)
        .max(999)
        .saturating_add(1)
}

pub fn descriptor_for_provider_and_model(
    settings: &BackendSettings,
    provider_id: &str,
    routing_slug: &str,
) -> Option<ModelRouteDescriptor> {
    model_route_descriptors(settings)
        .into_iter()
        .find(|descriptor| {
            descriptor.provider_id.eq_ignore_ascii_case(provider_id)
                && descriptor.routing_slug.eq_ignore_ascii_case(routing_slug)
        })
}

pub fn source_profile_for_descriptor<'a>(
    settings: &'a BackendSettings,
    descriptor: &ModelRouteDescriptor,
) -> Option<&'a crate::settings::RelayProfile> {
    match descriptor.source_kind {
        OfficialExperienceSourceKind::CliOfficial => {
            settings.relay_profiles.iter().find(|profile| {
                profile.id == descriptor.source_id
                    && profile
                        .integration_type
                        .eq_ignore_ascii_case(CLIPROXY_OFFICIAL_INTEGRATION_TYPE)
            })
        }
        OfficialExperienceSourceKind::CliGeneral => {
            settings.relay_profiles.iter().find(|profile| {
                profile.id == descriptor.source_id
                    && (profile.id == CLIPROXY_GENERAL_PROFILE_ID
                        || profile.integration_type.eq_ignore_ascii_case("cliproxy"))
            })
        }
        OfficialExperienceSourceKind::Relay => settings.relay_profiles.iter().find(|profile| {
            profile.id == descriptor.source_id
                && matches!(profile.relay_mode, RelayMode::MixedApi | RelayMode::PureApi)
                && !aggregate_model_alias::integration_is_excluded_from_aggregate(
                    &profile.integration_type,
                )
        }),
        _ => None,
    }
}

pub fn build_model_catalog(settings: &BackendSettings) -> Value {
    let mut models = crate::official_model_catalog::official_model_entries_for_settings(settings);
    let mut priority = extension_model_priority_start(settings);
    for descriptor in model_route_descriptors(settings) {
        if descriptor.source_kind == OfficialExperienceSourceKind::MainOfficial {
            continue;
        }
        let template = if descriptor.source_kind == OfficialExperienceSourceKind::DirectOfficial {
            crate::official_model_catalog::cached_official_model_entries_for_account(
                &descriptor.source_id,
            )
            .and_then(|entries| {
                entries.into_iter().find(|entry| {
                    entry
                        .get("slug")
                        .and_then(Value::as_str)
                        .is_some_and(|slug| slug.eq_ignore_ascii_case(&descriptor.capability_slug))
                })
            })
        } else {
            crate::official_model_catalog::official_model_metadata_for_settings(
                settings,
                &descriptor.capability_slug,
            )
        };
        let Some(mut entry) = template else {
            continue;
        };
        entry["slug"] = json!(descriptor.routing_slug);
        entry["display_name"] = json!(descriptor.display_name);
        entry["priority"] = json!(priority);
        priority = priority.saturating_add(1);
        entry["visibility"] = json!("hide");
        entry["prefer_websockets"] = json!(false);
        crate::model_suffix::apply_default_native_tool_mode(&mut entry);
        models.push(entry);
    }
    json!({ "models": models })
}

#[cfg(test)]
mod direct_official_tests {
    use super::*;

    #[test]
    fn account_routes_keep_identity_and_only_expose_visible_supported_models() {
        let trusted = HashSet::from(["gpt-6-luna".to_string(), "gpt-6-sol".to_string()]);
        let entries = vec![
            json!({"slug": "gpt-6-luna", "visibility": "list", "supported_in_api": true}),
            json!({"slug": "gpt-6-sol", "visibility": "hide", "supported_in_api": true}),
            json!({"slug": "gpt-6-sol", "visibility": "list", "supported_in_api": false}),
            json!({"slug": "untrusted-model", "visibility": "list", "supported_in_api": true}),
        ];
        let routes =
            direct_official_descriptors_for_account("account-2", "官方账号 2", &entries, &trusted);
        assert_eq!(routes.len(), 1);
        let route = &routes[0];
        assert_eq!(
            route.routing_slug,
            "codex_plus_official_account_2d_2:gpt-6-luna"
        );
        assert_eq!(route.display_name, "gpt-6-luna(官方账号 2)");
        assert_eq!(route.provider_id, "codex_plus_official_account_2d_2");
        assert_eq!(route.source_id, "account-2");
        assert_eq!(route.upstream_model, "gpt-6-luna");
        assert_eq!(route.failover_policy, "strict");
    }
}
