use codex_plus_core::relay_rotation::{
    MixedModelRoute, RelayRotationSelector, RotationContext, RotationEvent, SelectionError,
    classify_mixed_model_route, fallback_relays_after, record_relay_request_failure,
    select_dedicated_relay_for_model, select_relay_for_probe, select_relay_for_request,
};
use codex_plus_core::settings::{
    AggregateRelayDispatchTarget, AggregateRelayMember, AggregateRelayModelMapping,
    AggregateRelayProfile, AggregateRelayStrategy, BackendSettings, RelayMode, RelayProfile,
    RelaySessionProvider,
};
use std::sync::{Mutex, MutexGuard, OnceLock};

fn global_selector_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn profile(id: &str) -> RelayProfile {
    RelayProfile {
        id: id.to_string(),
        name: id.to_string(),
        base_url: format!("https://{id}.example/v1"),
        api_key: format!("sk-{id}"),
        ..RelayProfile::default()
    }
}

fn aggregate(strategy: AggregateRelayStrategy) -> AggregateRelayProfile {
    AggregateRelayProfile {
        id: "agg".to_string(),
        name: "聚合".to_string(),
        session_provider: RelaySessionProvider::Custom,
        strategy,
        model_mappings_enabled: true,
        members: vec![
            AggregateRelayMember {
                relay_id: "relay-a".to_string(),
                weight: 1,
            },
            AggregateRelayMember {
                relay_id: "relay-b".to_string(),
                weight: 2,
            },
            AggregateRelayMember {
                relay_id: "relay-c".to_string(),
                weight: 1,
            },
        ],
        model_mappings: Vec::new(),
    }
}

fn aggregate_with_id(id: &str, strategy: AggregateRelayStrategy) -> AggregateRelayProfile {
    AggregateRelayProfile {
        id: id.to_string(),
        name: "聚合".to_string(),
        session_provider: RelaySessionProvider::Custom,
        strategy,
        model_mappings_enabled: true,
        members: vec![
            AggregateRelayMember {
                relay_id: "relay-a".to_string(),
                weight: 1,
            },
            AggregateRelayMember {
                relay_id: "relay-b".to_string(),
                weight: 2,
            },
        ],
        model_mappings: Vec::new(),
    }
}

fn settings(strategy: AggregateRelayStrategy) -> BackendSettings {
    BackendSettings {
        relay_profiles: vec![
            profile("relay-a"),
            profile("relay-b"),
            profile("relay-c"),
            RelayProfile {
                id: "agg".to_string(),
                name: "聚合".to_string(),
                relay_mode: RelayMode::Aggregate,
                ..RelayProfile::default()
            },
        ],
        aggregate_relay_profiles: vec![aggregate(strategy)],
        active_relay_id: "agg".to_string(),
        active_aggregate_relay_id: "agg".to_string(),
        ..BackendSettings::default()
    }
}

#[test]
fn provider_specific_gpt_alias_selects_only_the_requested_member() {
    let mut settings = settings(AggregateRelayStrategy::Failover);
    settings.relay_profiles[0].name = "ProviderA".to_string();
    settings.relay_profiles[0].model = "gpt-5.4".to_string();
    settings.relay_profiles[0].model_list = "gpt-5.4".to_string();
    settings.relay_profiles[1].name = "ProviderB".to_string();
    settings.relay_profiles[1].model = "vendor-gpt-5.4".to_string();
    settings.relay_profiles[1].model_list = "vendor-gpt-5.4".to_string();
    settings.aggregate_relay_profiles[0].model_mappings = vec![AggregateRelayModelMapping {
        codex_model: "gpt-5.4".to_string(),
        targets: vec![
            AggregateRelayDispatchTarget {
                relay_id: "relay-a".to_string(),
                target_model: "gpt-5.4".to_string(),
            },
            AggregateRelayDispatchTarget {
                relay_id: "relay-b".to_string(),
                target_model: "vendor-gpt-5.4".to_string(),
            },
        ],
    }];
    let mut selector = RelayRotationSelector::from_settings(&settings).unwrap();

    let selected = selector
        .select(
            &settings,
            RotationContext::default(),
            Some("ProviderB:vendor-gpt-5.4"),
        )
        .unwrap();

    assert_eq!(selected.id, "relay-b");
}

#[test]
fn cliproxy_official_channel_is_rejected_as_an_aggregate_member() {
    let mut settings = settings(AggregateRelayStrategy::Failover);
    settings.relay_profiles[0].id = "managed-cliproxy-official".to_string();
    settings.relay_profiles[0].integration_type = "cliproxy-official".to_string();
    settings.aggregate_relay_profiles[0].members[0].relay_id =
        "managed-cliproxy-official".to_string();

    match RelayRotationSelector::from_settings(&settings) {
        Err(error) => assert_eq!(
            error,
            SelectionError::ExcludedMemberRelay {
                aggregate_id: "agg".to_string(),
                relay_id: "managed-cliproxy-official".to_string(),
            }
        ),
        Ok(_) => panic!("专用官方通道不应成为聚合成员"),
    }
}

#[test]
fn cliproxy_integration_is_rejected_as_an_aggregate_member() {
    let mut settings = settings(AggregateRelayStrategy::Failover);
    settings.relay_profiles[0].id = "managed-cliproxy".to_string();
    settings.relay_profiles[0].integration_type = "cliproxy".to_string();
    settings.aggregate_relay_profiles[0].members[0].relay_id = "managed-cliproxy".to_string();

    match RelayRotationSelector::from_settings(&settings) {
        Err(error) => assert_eq!(
            error,
            SelectionError::ExcludedMemberRelay {
                aggregate_id: "agg".to_string(),
                relay_id: "managed-cliproxy".to_string(),
            }
        ),
        Ok(_) => panic!("CLIProxyAPI 接入不应成为聚合成员"),
    }
}

#[test]
fn provider_specific_gpt_alias_selects_requested_member_when_implicit_mappings_are_disabled() {
    let mut settings = settings(AggregateRelayStrategy::Failover);
    settings.relay_profiles[0].name = "ProviderA".to_string();
    settings.relay_profiles[0].model = "gpt-5.4".to_string();
    settings.relay_profiles[0].model_list = "gpt-5.4".to_string();
    settings.relay_profiles[1].name = "ProviderB".to_string();
    settings.relay_profiles[1].model = "gpt-5.4".to_string();
    settings.relay_profiles[1].model_list = "gpt-5.4".to_string();
    settings.aggregate_relay_profiles[0].model_mappings_enabled = false;
    settings.aggregate_relay_profiles[0].model_mappings.clear();
    let mut selector = RelayRotationSelector::from_settings(&settings).unwrap();

    let selected = selector
        .select(
            &settings,
            RotationContext::default(),
            Some("ProviderB:gpt-5.4"),
        )
        .unwrap();

    assert_eq!(selected.id, "relay-b");
}

#[test]
fn official_mixed_mode_classifies_models_without_supplier_leakage() {
    let mut settings = settings(AggregateRelayStrategy::Failover);
    settings.official_login_mixed_mode = true;
    settings.official_login_relay_id = "official".to_string();
    settings.relay_profiles.push(RelayProfile {
        id: "official".to_string(),
        name: "OpenAI".to_string(),
        relay_mode: RelayMode::Official,
        auth_contents:
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"token","account_id":"account"}}"#
                .to_string(),
        ..RelayProfile::default()
    });
    settings.relay_profiles[0].name = "ProviderA".to_string();
    settings.relay_profiles[0].model_list = "gpt-5.6-sol\ngpt-5.2".to_string();

    assert_eq!(
        classify_mixed_model_route(&settings, Some("gpt-5.6-sol")),
        MixedModelRoute::Official
    );
    assert_eq!(
        classify_mixed_model_route(&settings, Some("gpt-5.6-sol(ProviderA)")),
        MixedModelRoute::Aggregate
    );
    assert_eq!(
        classify_mixed_model_route(&settings, Some("ProviderA:gpt-5.6-sol")),
        MixedModelRoute::Aggregate
    );
    assert_eq!(
        classify_mixed_model_route(&settings, Some("gpt-5.2")),
        MixedModelRoute::Reject
    );
    assert_eq!(
        classify_mixed_model_route(&settings, Some("unknown:model")),
        MixedModelRoute::Reject
    );
}

#[test]
fn cliproxy_official_alias_uses_dedicated_relay_and_keeps_raw_target_model() {
    let mut settings = settings(AggregateRelayStrategy::Failover);
    settings.official_login_mixed_mode = true;
    settings.official_login_relay_id = "official".to_string();
    settings.relay_profiles.push(RelayProfile {
        id: "official".to_string(),
        name: "OpenAI".to_string(),
        relay_mode: RelayMode::Official,
        auth_contents:
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"token","account_id":"account"}}"#
                .to_string(),
        ..RelayProfile::default()
    });
    settings.relay_profiles.push(RelayProfile {
        id: "managed-cliproxy-official".to_string(),
        name: "CLIProxyAPI 官方模型".to_string(),
        integration_type: "cliproxy-official".to_string(),
        base_url: "http://127.0.0.1:8317/v1".to_string(),
        api_key: "cli-key".to_string(),
        model: "account-2/gpt-5.6-sol".to_string(),
        model_list: "account-2/gpt-5.6-sol".to_string(),
        ..RelayProfile::default()
    });
    settings.relay_profiles.push(RelayProfile {
        id: "managed-cliproxy".to_string(),
        name: "CLIProxyAPI".to_string(),
        integration_type: "cliproxy".to_string(),
        base_url: "http://127.0.0.1:8317/v1".to_string(),
        api_key: "cli-key".to_string(),
        model: "gemini-2.5-pro".to_string(),
        model_list: "account-2/gpt-5.6-sol\ngemini-2.5-pro".to_string(),
        ..RelayProfile::default()
    });

    assert_eq!(
        classify_mixed_model_route(&settings, Some("CLIProxyAPI:gpt-5.6-sol")),
        MixedModelRoute::DedicatedRelay
    );
    let relay =
        select_dedicated_relay_for_model(&settings, Some("CLIProxyAPI:gpt-5.6-sol")).unwrap();
    assert_eq!(relay.id, "managed-cliproxy-official");
    assert_eq!(
        relay
            .model_mappings
            .get("CLIProxyAPI:gpt-5.6-sol")
            .map(String::as_str),
        Some("account-2/gpt-5.6-sol")
    );
    assert_eq!(
        classify_mixed_model_route(&settings, Some("CLIProxyAPI:gemini-2.5-pro")),
        MixedModelRoute::DedicatedRelay
    );
    let relay =
        select_dedicated_relay_for_model(&settings, Some("CLIProxyAPI:gemini-2.5-pro")).unwrap();
    assert_eq!(relay.id, "managed-cliproxy");
    assert_eq!(
        relay
            .model_mappings
            .get("CLIProxyAPI:gemini-2.5-pro")
            .map(String::as_str),
        Some("gemini-2.5-pro")
    );
}

#[test]
fn cliproxy_general_alias_remains_direct_without_official_auth() {
    let mut settings = settings(AggregateRelayStrategy::Failover);
    settings.relay_profiles.push(RelayProfile {
        id: "managed-cliproxy".to_string(),
        name: "CLIProxyAPI".to_string(),
        integration_type: "cliproxy".to_string(),
        base_url: "http://127.0.0.1:8317/v1".to_string(),
        api_key: "cli-key".to_string(),
        model: "gemini-2.5-pro".to_string(),
        model_list: "gpt-5.6-sol\ngemini-2.5-pro".to_string(),
        ..RelayProfile::default()
    });

    assert_eq!(
        classify_mixed_model_route(&settings, Some("CLIProxyAPI:gpt-5.6-sol")),
        MixedModelRoute::DedicatedRelay
    );
    assert_eq!(
        classify_mixed_model_route(&settings, Some("CLIProxyAPI:gemini-2.5-pro")),
        MixedModelRoute::DedicatedRelay
    );
    let relay =
        select_dedicated_relay_for_model(&settings, Some("CLIProxyAPI:gemini-2.5-pro")).unwrap();
    assert_eq!(relay.id, "managed-cliproxy");
}

#[test]
fn combined_aggregate_alias_keeps_all_explicit_mapping_targets() {
    let mut settings = settings(AggregateRelayStrategy::Failover);
    settings.relay_profiles[0].name = "ProviderA".to_string();
    settings.relay_profiles[0].model_list = "gpt-5.4".to_string();
    settings.relay_profiles[1].name = "ProviderB".to_string();
    settings.relay_profiles[1].model_list = "vendor-gpt-5.4".to_string();
    settings.aggregate_relay_profiles[0].model_mappings = vec![AggregateRelayModelMapping {
        codex_model: "gpt-5.4".to_string(),
        targets: vec![
            AggregateRelayDispatchTarget {
                relay_id: "relay-a".to_string(),
                target_model: "gpt-5.4".to_string(),
            },
            AggregateRelayDispatchTarget {
                relay_id: "relay-b".to_string(),
                target_model: "vendor-gpt-5.4".to_string(),
            },
        ],
    }];
    let alias = "gpt-5.4(ProviderA|ProviderB:vendor-gpt-5.4)";

    let first =
        select_relay_for_request(&settings, RotationContext::default(), Some(alias)).unwrap();
    let fallbacks = fallback_relays_after(&settings, &first.id, Some(alias)).unwrap();

    assert_eq!(first.id, "relay-a");
    assert_eq!(
        fallbacks
            .iter()
            .map(|relay| relay.id.as_str())
            .collect::<Vec<_>>(),
        ["relay-b"]
    );
}

#[test]
fn failover_keeps_current_provider_until_failure_then_moves_to_next_member() {
    let settings = settings(AggregateRelayStrategy::Failover);
    let mut selector = RelayRotationSelector::from_settings(&settings).unwrap();

    let first = selector
        .select(&settings, RotationContext::for_conversation("chat-1"), None)
        .unwrap();
    selector.record_event(RotationEvent::Success);
    let second = selector
        .select(&settings, RotationContext::for_conversation("chat-1"), None)
        .unwrap();
    selector.record_event(RotationEvent::Failure);
    let third = selector
        .select(&settings, RotationContext::for_conversation("chat-1"), None)
        .unwrap();

    assert_eq!(first.id, "relay-a");
    assert_eq!(second.id, "relay-a");
    assert_eq!(third.id, "relay-b");
}

#[test]
fn conversation_rotation_sticks_each_conversation_to_a_stable_member() {
    let settings = settings(AggregateRelayStrategy::ConversationRoundRobin);
    let mut selector = RelayRotationSelector::from_settings(&settings).unwrap();

    let chat_a_first = selector
        .select(&settings, RotationContext::for_conversation("chat-a"), None)
        .unwrap();
    let chat_a_second = selector
        .select(&settings, RotationContext::for_conversation("chat-a"), None)
        .unwrap();
    let chat_b_first = selector
        .select(&settings, RotationContext::for_conversation("chat-b"), None)
        .unwrap();

    assert_eq!(chat_a_first.id, "relay-a");
    assert_eq!(chat_a_second.id, "relay-a");
    assert_eq!(chat_b_first.id, "relay-b");
}

#[test]
fn request_rotation_advances_on_every_request() {
    let settings = settings(AggregateRelayStrategy::RequestRoundRobin);
    let mut selector = RelayRotationSelector::from_settings(&settings).unwrap();

    let selected = (0..5)
        .map(|_| {
            selector
                .select(&settings, RotationContext::default(), None)
                .unwrap()
                .id
        })
        .collect::<Vec<_>>();

    assert_eq!(
        selected,
        vec!["relay-a", "relay-b", "relay-c", "relay-a", "relay-b"]
    );
}

#[test]
fn weighted_rotation_repeats_members_by_configured_weight() {
    let settings = settings(AggregateRelayStrategy::WeightedRoundRobin);
    let mut selector = RelayRotationSelector::from_settings(&settings).unwrap();

    let selected = (0..6)
        .map(|_| {
            selector
                .select(&settings, RotationContext::default(), None)
                .unwrap()
                .id
        })
        .collect::<Vec<_>>();

    assert_eq!(
        selected,
        vec![
            "relay-a", "relay-b", "relay-b", "relay-c", "relay-a", "relay-b"
        ]
    );
}

#[test]
fn aggregate_members_must_reference_existing_relay_profiles() {
    let mut settings = settings(AggregateRelayStrategy::RequestRoundRobin);
    settings.aggregate_relay_profiles[0]
        .members
        .push(AggregateRelayMember {
            relay_id: "missing-relay".to_string(),
            weight: 1,
        });

    let error = RelayRotationSelector::from_settings(&settings).unwrap_err();

    assert_eq!(
        error,
        SelectionError::UnknownMemberRelay {
            aggregate_id: "agg".to_string(),
            relay_id: "missing-relay".to_string()
        }
    );
}

#[test]
fn aggregate_with_one_member_is_allowed_without_rotation() {
    let mut settings = settings(AggregateRelayStrategy::RequestRoundRobin);
    settings.aggregate_relay_profiles[0].members.truncate(1);

    let mut selector = RelayRotationSelector::from_settings(&settings).unwrap();
    let first = selector
        .select(&settings, RotationContext::default(), None)
        .unwrap();
    let second = selector
        .select(&settings, RotationContext::default(), None)
        .unwrap();

    assert_eq!(first.id, "relay-a");
    assert_eq!(second.id, "relay-a");
}

#[test]
fn aggregate_members_must_be_api_capable_relay_profiles() {
    let mut settings = settings(AggregateRelayStrategy::WeightedRoundRobin);
    settings.relay_profiles.push(RelayProfile {
        id: "official-login".to_string(),
        name: "官方登录".to_string(),
        base_url: String::new(),
        api_key: String::new(),
        ..RelayProfile::default()
    });
    settings.aggregate_relay_profiles[0]
        .members
        .push(AggregateRelayMember {
            relay_id: "official-login".to_string(),
            weight: 1,
        });

    let error = RelayRotationSelector::from_settings(&settings).unwrap_err();

    assert_eq!(
        error,
        SelectionError::InvalidMemberRelay {
            aggregate_id: "agg".to_string(),
            relay_id: "official-login".to_string()
        }
    );
}

#[test]
fn select_relay_for_request_uses_active_relay_id_as_aggregate_source_of_truth() {
    let _guard = global_selector_test_lock();
    let mut settings = settings(AggregateRelayStrategy::WeightedRoundRobin);
    settings.active_relay_id = "agg".to_string();
    settings.active_aggregate_relay_id.clear();

    let selected = select_relay_for_request(&settings, RotationContext::default(), None).unwrap();

    assert_eq!(selected.id, "relay-a");
}

#[test]
fn select_relay_for_request_ignores_stale_active_aggregate_id_for_regular_relay() {
    let _guard = global_selector_test_lock();
    let mut settings = settings(AggregateRelayStrategy::WeightedRoundRobin);
    settings.active_relay_id = "relay-b".to_string();
    settings.active_aggregate_relay_id = "agg".to_string();

    let selected = select_relay_for_request(&settings, RotationContext::default(), None).unwrap();

    assert_eq!(selected.id, "relay-b");
}

#[test]
fn select_relay_for_request_resets_rotation_after_switching_to_regular_relay() {
    let _guard = global_selector_test_lock();
    let mut settings = settings(AggregateRelayStrategy::RequestRoundRobin);
    settings.active_relay_id = "agg".to_string();

    let first = select_relay_for_request(&settings, RotationContext::default(), None).unwrap();
    let mut regular_settings = settings.clone();
    regular_settings.active_relay_id = "relay-c".to_string();
    regular_settings.active_aggregate_relay_id.clear();
    let regular =
        select_relay_for_request(&regular_settings, RotationContext::default(), None).unwrap();
    let after_reselect =
        select_relay_for_request(&settings, RotationContext::default(), None).unwrap();

    assert_eq!(first.id, "relay-a");
    assert_eq!(regular.id, "relay-c");
    assert_eq!(after_reselect.id, "relay-a");
}

#[test]
fn record_relay_request_failure_advances_global_failover_selector() {
    let _guard = global_selector_test_lock();
    let aggregate_id = "agg-global-failure";
    let settings = BackendSettings {
        relay_profiles: vec![
            profile("relay-a"),
            profile("relay-b"),
            RelayProfile {
                id: aggregate_id.to_string(),
                name: "聚合".to_string(),
                relay_mode: RelayMode::Aggregate,
                ..RelayProfile::default()
            },
        ],
        aggregate_relay_profiles: vec![aggregate_with_id(
            aggregate_id,
            AggregateRelayStrategy::Failover,
        )],
        active_relay_id: aggregate_id.to_string(),
        active_aggregate_relay_id: aggregate_id.to_string(),
        ..BackendSettings::default()
    };

    let first = select_relay_for_request(&settings, RotationContext::default(), None).unwrap();
    record_relay_request_failure(&settings);
    let second = select_relay_for_request(&settings, RotationContext::default(), None).unwrap();

    assert_eq!(first.id, "relay-a");
    assert_eq!(second.id, "relay-b");
}

#[test]
fn select_relay_for_probe_does_not_advance_request_rotation() {
    let _guard = global_selector_test_lock();
    let aggregate_id = "agg-probe";
    let settings = BackendSettings {
        relay_profiles: vec![
            profile("relay-a"),
            profile("relay-b"),
            RelayProfile {
                id: aggregate_id.to_string(),
                name: "聚合".to_string(),
                relay_mode: RelayMode::Aggregate,
                ..RelayProfile::default()
            },
        ],
        aggregate_relay_profiles: vec![aggregate_with_id(
            aggregate_id,
            AggregateRelayStrategy::RequestRoundRobin,
        )],
        active_relay_id: aggregate_id.to_string(),
        active_aggregate_relay_id: aggregate_id.to_string(),
        ..BackendSettings::default()
    };

    let first_probe = select_relay_for_probe(&settings, None).unwrap();
    let second_probe = select_relay_for_probe(&settings, None).unwrap();
    let first_request =
        select_relay_for_request(&settings, RotationContext::default(), None).unwrap();
    let second_request =
        select_relay_for_request(&settings, RotationContext::default(), None).unwrap();

    assert_eq!(first_probe.id, "relay-a");
    assert_eq!(second_probe.id, "relay-a");
    assert_eq!(first_request.id, "relay-a");
    assert_eq!(second_request.id, "relay-b");
}

#[test]
fn fallback_relays_after_returns_remaining_aggregate_members_after_current_then_wraps() {
    let settings = settings(AggregateRelayStrategy::RequestRoundRobin);

    let fallbacks = fallback_relays_after(&settings, "relay-b", None).unwrap();

    assert_eq!(
        fallbacks
            .iter()
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>(),
        vec!["relay-c", "relay-a"]
    );
}

#[test]
fn fallback_relays_after_regular_relay_returns_empty_candidates() {
    let mut settings = settings(AggregateRelayStrategy::RequestRoundRobin);
    settings.active_relay_id = "relay-a".to_string();

    let fallbacks = fallback_relays_after(&settings, "relay-a", None).unwrap();

    assert!(fallbacks.is_empty());
}

#[test]
fn select_relay_for_request_rebuilds_selector_when_active_aggregate_changes() {
    let _guard = global_selector_test_lock();
    let aggregate_id = "agg-refresh";
    let mut settings = BackendSettings {
        relay_profiles: vec![
            profile("relay-a"),
            profile("relay-b"),
            RelayProfile {
                id: aggregate_id.to_string(),
                name: "聚合".to_string(),
                relay_mode: RelayMode::Aggregate,
                ..RelayProfile::default()
            },
        ],
        aggregate_relay_profiles: vec![aggregate_with_id(
            aggregate_id,
            AggregateRelayStrategy::Failover,
        )],
        active_relay_id: aggregate_id.to_string(),
        active_aggregate_relay_id: aggregate_id.to_string(),
        ..BackendSettings::default()
    };

    let first = select_relay_for_request(&settings, RotationContext::default(), None).unwrap();
    settings.aggregate_relay_profiles[0].strategy = AggregateRelayStrategy::WeightedRoundRobin;

    let selected = (0..3)
        .map(|_| {
            select_relay_for_request(&settings, RotationContext::default(), None)
                .unwrap()
                .id
        })
        .collect::<Vec<_>>();

    assert_eq!(first.id, "relay-a");
    assert_eq!(selected, vec!["relay-a", "relay-b", "relay-b"]);
}
