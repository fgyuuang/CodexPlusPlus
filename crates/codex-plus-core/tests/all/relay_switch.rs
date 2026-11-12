use codex_plus_core::relay_switch::{
    switch_official_account_in_home, switch_relay_profile_in_home,
};
use codex_plus_core::settings::{
    AggregateRelayMember, AggregateRelayProfile, AggregateRelayStrategy, BackendSettings,
    RelayMode, RelayProfile, RelaySessionProvider, SettingsStore,
};

#[test]
fn switch_rolls_back_active_settings_when_live_write_fails() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let original = BackendSettings {
        active_relay_id: "a".to_string(),
        relay_profiles: vec![pure_profile("a", "https://a.example/v1", "sk-a")],
        ..BackendSettings::default()
    };
    store.save(&original).unwrap();
    std::fs::create_dir(temp.path().join("codex")).unwrap();
    std::fs::write(
        temp.path().join("codex").join("auth.json"),
        r#"{"OPENAI_API_KEY":"sk-a"}"#,
    )
    .unwrap();
    std::fs::write(
        temp.path().join("codex").join("config.toml"),
        r#"model_provider = "custom"

[model_providers.custom]
name = "custom"
wire_api = "responses"
requires_openai_auth = true
base_url = "https://a.example/v1"
"#,
    )
    .unwrap();
    let next = BackendSettings {
        settings_revision: store.load().unwrap().settings_revision,
        active_relay_id: "b".to_string(),
        relay_profiles: vec![
            pure_profile("a", "https://a.example/v1", "sk-a"),
            RelayProfile {
                id: "b".to_string(),
                name: "B".to_string(),
                relay_mode: RelayMode::PureApi,
                config_contents: "model_provider = \"custom\"\n".to_string(),
                auth_contents: "{bad json".to_string(),
                ..RelayProfile::default()
            },
        ],
        ..BackendSettings::default()
    };

    let error = switch_relay_profile_in_home(&store, &temp.path().join("codex"), next, "a")
        .expect_err("missing API key should fail switch status validation");

    assert!(
        error
            .to_string()
            .contains("纯 API 配置写入后未检测到完整 custom provider")
    );
    assert_eq!(store.load().unwrap().active_relay_id, "a");
    assert!(
        std::fs::read_to_string(temp.path().join("codex").join("config.toml"))
            .unwrap()
            .contains("https://a.example/v1")
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("codex").join("auth.json")).unwrap(),
        r#"{"OPENAI_API_KEY":"sk-a"}"#
    );
}

#[test]
fn switch_rolls_back_live_files_when_post_write_status_check_fails() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    let original_auth = r#"{"OPENAI_API_KEY":"sk-a"}"#;
    let original_config = r#"model_provider = "custom"

[hooks.state."plugin-a@personal:hooks/hooks.json:pre_tool_use:0:0"]
trusted_hash = "live-a-hash"

[hooks.state."plugin-b@openai-bundled:hooks/hooks.json:user_prompt_submit:1:0"]
trusted_hash = "live-b-hash"

[model_providers.custom]
name = "custom"
wire_api = "responses"
requires_openai_auth = true
base_url = "https://a.example/v1"
"#;
    std::fs::write(home.join("auth.json"), original_auth).unwrap();
    std::fs::write(home.join("config.toml"), original_config).unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let original = BackendSettings {
        active_relay_id: "a".to_string(),
        relay_profiles: vec![pure_profile("a", "https://a.example/v1", "sk-a")],
        ..BackendSettings::default()
    };
    store.save(&original).unwrap();
    let persisted_original = store.load().unwrap();
    let original_settings_bytes = std::fs::read(temp.path().join("settings.json")).unwrap();
    let next = BackendSettings {
        active_relay_id: "b".to_string(),
        relay_profiles: vec![
            pure_profile("a", "https://a.example/v1", "sk-a"),
            RelayProfile {
                id: "b".to_string(),
                name: "B".to_string(),
                relay_mode: RelayMode::PureApi,
                config_contents: r#"model_provider = "custom"

[model_providers.custom]
name = "custom"
wire_api = "responses"
requires_openai_auth = true
base_url = "https://b.example/v1"
"#
                .to_string(),
                auth_contents: "{}".to_string(),
                ..RelayProfile::default()
            },
        ],
        ..BackendSettings::default()
    };

    let error = switch_current_relay_profile(&store, &home, next, "a")
        .expect_err("missing api key should fail post-write status check");

    assert!(
        error
            .to_string()
            .contains("纯 API 配置写入后未检测到完整 custom provider")
    );
    assert_eq!(store.load().unwrap(), persisted_original);
    assert_eq!(
        std::fs::read(temp.path().join("settings.json")).unwrap(),
        original_settings_bytes
    );
    assert_eq!(
        std::fs::read_to_string(home.join("config.toml")).unwrap(),
        original_config
    );
    assert_eq!(
        std::fs::read_to_string(home.join("auth.json")).unwrap(),
        original_auth
    );
}

#[test]
fn switch_preserves_previous_profile_when_live_config_changes_before_selecting_target() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        r#"model = "edited-live-model"
model_provider = "manual_a"
model_context_window = 1000000
model_auto_compact_token_limit = 900000

[hooks.state."plugin-a@personal:hooks/hooks.json:pre_tool_use:0:0"]
trusted_hash = "live-a-hash"

[hooks.state."plugin-b@openai-bundled:hooks/hooks.json:user_prompt_submit:1:0"]
trusted_hash = "live-b-hash"

[model_providers.manual_a]
name = "manual_a"
wire_api = "responses"
requires_openai_auth = true
base_url = "https://edited-a.example/v1"
"#,
    )
    .unwrap();
    std::fs::write(
        home.join("auth.json"),
        r#"{"OPENAI_API_KEY":"sk-edited-a"}"#,
    )
    .unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let original = BackendSettings {
        active_relay_id: "a".to_string(),
        relay_profiles: vec![
            pure_profile("a", "https://edited-a.example/v1", "sk-a"),
            pure_profile("b", "https://b.example/v1", "sk-b"),
        ],
        ..BackendSettings::default()
    };
    store.save(&original).unwrap();
    let next = BackendSettings {
        active_relay_id: "b".to_string(),
        relay_profiles: original.relay_profiles.clone(),
        ..BackendSettings::default()
    };

    switch_current_relay_profile(&store, &home, next, "a").unwrap();

    let stored = store.load().unwrap();
    let previous = stored
        .relay_profiles
        .iter()
        .find(|profile| profile.id == "a")
        .unwrap();
    assert!(!previous.config_contents.contains("edited-live-model"));
    assert!(
        previous
            .config_contents
            .contains(r#"model_provider = "custom""#)
    );
    assert!(
        previous
            .config_contents
            .contains("[model_providers.custom]")
    );
    assert_eq!(previous.auth_contents, r#"{"OPENAI_API_KEY":"sk-a"}"#);
    assert_eq!(previous.api_key, "sk-a");
    assert!(previous.context_window.is_empty());
    assert!(previous.auto_compact_limit.is_empty());
    assert_eq!(stored.active_relay_id, "b");
    assert!(
        serde_json::to_value(&stored)
            .unwrap()
            .get("launchMode")
            .is_none()
    );
    let live: toml::Value = std::fs::read_to_string(home.join("config.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        live["hooks"]["state"]["plugin-a@personal:hooks/hooks.json:pre_tool_use:0:0"]
            ["trusted_hash"]
            .as_str(),
        Some("live-a-hash")
    );
    assert_eq!(
        live["hooks"]["state"]["plugin-b@openai-bundled:hooks/hooks.json:user_prompt_submit:1:0"]
            ["trusted_hash"]
            .as_str(),
        Some("live-b-hash")
    );
}

#[test]
fn switch_does_not_backfill_previous_profile_from_aggregate_live_config() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        r#"model_provider = "custom"

[model_providers.custom]
name = "custom"
wire_api = "responses"
requires_openai_auth = false
base_url = "http://127.0.0.1:57321/v1"
experimental_bearer_token = "codex-plus-aggregate"
"#,
    )
    .unwrap();
    std::fs::write(home.join("auth.json"), r#"{"OPENAI_API_KEY":"1234"}"#).unwrap();

    let store = SettingsStore::new(temp.path().join("settings.json"));
    let previous = pure_profile("a", "https://a.example/v1", "sk-a");
    let original = BackendSettings {
        active_relay_id: "a".to_string(),
        relay_profiles: vec![
            previous.clone(),
            pure_profile("b", "https://b.example/v1", "sk-b"),
        ],
        ..BackendSettings::default()
    };
    store.save(&original).unwrap();
    let next = BackendSettings {
        active_relay_id: "b".to_string(),
        relay_profiles: original.relay_profiles.clone(),
        ..store.load().unwrap()
    };

    switch_relay_profile_in_home(&store, &home, next, "a").unwrap();

    let stored_previous = store
        .load()
        .unwrap()
        .relay_profiles
        .into_iter()
        .find(|profile| profile.id == "a")
        .unwrap();
    assert!(
        stored_previous
            .config_contents
            .contains("https://a.example/v1")
    );
    assert!(stored_previous.config_contents.contains("sk-a"));
    assert!(!stored_previous.config_contents.contains("1234"));
    assert!(
        !stored_previous
            .config_contents
            .contains("codex-plus-aggregate")
    );
    assert_eq!(stored_previous.auth_contents, previous.auth_contents);
}

#[test]
fn switch_does_not_backfill_previous_profile_from_a_different_provider() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        r#"model_provider = "custom"

[model_providers.custom]
name = "custom"
wire_api = "responses"
requires_openai_auth = false
base_url = "https://b.example/v1"
experimental_bearer_token = "sk-b"
"#,
    )
    .unwrap();
    std::fs::write(home.join("auth.json"), r#"{"OPENAI_API_KEY":"sk-b"}"#).unwrap();

    let store = SettingsStore::new(temp.path().join("settings.json"));
    let previous = pure_profile("a", "https://a.example/v1", "sk-a");
    let original = BackendSettings {
        active_relay_id: "a".to_string(),
        relay_profiles: vec![
            previous.clone(),
            pure_profile("b", "https://b.example/v1", "sk-b"),
        ],
        ..BackendSettings::default()
    };
    store.save(&original).unwrap();
    let next = BackendSettings {
        active_relay_id: "b".to_string(),
        relay_profiles: original.relay_profiles.clone(),
        ..store.load().unwrap()
    };

    switch_relay_profile_in_home(&store, &home, next, "a").unwrap();

    let stored_previous = store
        .load()
        .unwrap()
        .relay_profiles
        .into_iter()
        .find(|profile| profile.id == "a")
        .unwrap();
    assert!(
        stored_previous
            .config_contents
            .contains("https://a.example/v1")
    );
    assert!(stored_previous.config_contents.contains("sk-a"));
    assert!(!stored_previous.config_contents.contains("sk-b"));
    assert_eq!(stored_previous.auth_contents, previous.auth_contents);
}

#[test]
fn switch_to_aggregate_relay_allows_empty_config_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let api = pure_profile("api", "https://api.example/v1", "sk-api");
    let aggregate = RelayProfile {
        id: "agg".to_string(),
        name: "聚合供应商 1".to_string(),
        relay_mode: RelayMode::Aggregate,
        config_contents: String::new(),
        auth_contents: String::new(),
        ..RelayProfile::default()
    };
    let original = BackendSettings {
        active_relay_id: "api".to_string(),
        relay_profiles: vec![api.clone(), aggregate.clone()],
        ..BackendSettings::default()
    };
    store.save(&original).unwrap();
    let next = BackendSettings {
        active_relay_id: "agg".to_string(),
        relay_profiles: vec![api, aggregate],
        aggregate_relay_profiles: vec![AggregateRelayProfile {
            id: "agg".to_string(),
            name: "聚合供应商 1".to_string(),
            session_provider: RelaySessionProvider::Custom,
            model_aggregation_enabled: false,
            strategy: AggregateRelayStrategy::Failover,
            model_mappings_enabled: true,
            members: vec![AggregateRelayMember {
                relay_id: "api".to_string(),
                weight: 1,
            }],
            model_mappings: Vec::new(),
            routes: Vec::new(),
            default_relay_id: String::new(),
            sync_default_member_login: false,
            auto_review_relay_id: String::new(),
        }],
        active_aggregate_relay_id: "agg".to_string(),
        ..BackendSettings::default()
    };

    let result = switch_current_relay_profile(&store, &home, next, "api").unwrap();
    let live = std::fs::read_to_string(home.join("config.toml")).unwrap();

    assert!(result.configured);
    assert_eq!(store.load().unwrap().active_relay_id, "agg");
    assert!(live.contains(r#"base_url = "http://127.0.0.1:57321/v1""#));
}

#[test]
fn mixed_aggregate_target_preserves_auth_until_explicit_login_restore() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        home.join("auth.json"),
        r#"{"auth_mode":"apikey","OPENAI_API_KEY":"stale"}"#,
    )
    .unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let official_auth = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"official-account"}}"#;
    let official = RelayProfile {
        id: "official".to_string(),
        name: "OpenAI".to_string(),
        relay_mode: RelayMode::Official,
        auth_contents: official_auth.to_string(),
        ..RelayProfile::default()
    };
    let api = pure_profile("api", "https://api.example/v1", "sk-api");
    let aggregate = RelayProfile {
        id: "agg".to_string(),
        name: "聚合供应商".to_string(),
        relay_mode: RelayMode::Aggregate,
        model: "gpt-5.4".to_string(),
        ..RelayProfile::default()
    };
    let next = BackendSettings {
        active_relay_id: "agg".to_string(),
        relay_profiles: vec![official, api, aggregate],
        official_login_mixed_mode: true,
        official_login_relay_id: "official".to_string(),
        aggregate_relay_profiles: vec![AggregateRelayProfile {
            auto_review_relay_id: String::new(),
            id: "agg".to_string(),
            name: "聚合供应商".to_string(),
            strategy: AggregateRelayStrategy::Failover,
            session_provider: RelaySessionProvider::Custom,
            model_mappings_enabled: true,
            members: vec![AggregateRelayMember {
                relay_id: "api".to_string(),
                weight: 1,
            }],
            model_mappings: Vec::new(),
            routes: Vec::new(),
            model_aggregation_enabled: false,
            default_relay_id: String::new(),
            sync_default_member_login: false,
        }],
        active_aggregate_relay_id: "agg".to_string(),
        ..BackendSettings::default()
    };

    let before_auth = std::fs::read(home.join("auth.json")).ok();
    let ordinary = switch_relay_profile_in_home(&store, &home, next, "official").unwrap();
    assert_eq!(
        std::fs::read(home.join("auth.json")).ok(),
        before_auth,
        "选择API/聚合请求目标或兼容登录字段不授权改写启动登录"
    );
    let result =
        switch_official_account_in_home(&store, &home, ordinary.settings, official_auth).unwrap();
    let live_config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    let live_auth = std::fs::read_to_string(home.join("auth.json")).unwrap();

    assert!(result.configured);
    assert!(live_config.contains(r#"model_provider = "openai""#));
    let configured_model = live_config
        .lines()
        .find_map(|line| line.trim().strip_prefix("model = \"")?.strip_suffix('"'))
        .unwrap_or_default();
    assert!(!configured_model.is_empty());
    assert!(
        codex_plus_core::official_model_catalog::visible_official_model_slugs_for_settings(
            &result.settings,
        )
        .iter()
        .any(|model| model.eq_ignore_ascii_case(configured_model))
    );
    assert!(!live_config.contains(r#"experimental_bearer_token = "codex-plus-aggregate""#));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&live_auth).unwrap(),
        serde_json::from_str::<serde_json::Value>(official_auth).unwrap()
    );
    assert_eq!(result.settings.aggregate_relay_profiles[0].members.len(), 1);
    assert_eq!(
        result.settings.aggregate_relay_profiles[0].members[0].relay_id,
        "api"
    );
}

#[test]
fn mixed_single_target_preserves_absent_auth_until_explicit_login_restore() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let official = RelayProfile {
        id: "official".to_string(),
        name: "OpenAI".to_string(),
        relay_mode: RelayMode::Official,
        auth_contents: r#"{"auth_mode":"chatgpt","tokens":{"access_token":"official-account"}}"#
            .to_string(),
        ..RelayProfile::default()
    };
    let api = pure_profile("api", "https://api.example/v1", "sk-api");
    let next = BackendSettings {
        active_relay_id: "api".to_string(),
        relay_profiles: vec![official, api],
        official_login_mixed_mode: true,
        official_login_relay_id: "official".to_string(),
        ..BackendSettings::default()
    };

    let before_auth = std::fs::read(home.join("auth.json")).ok();
    let ordinary = switch_relay_profile_in_home(&store, &home, next, "official").unwrap();
    assert_eq!(
        std::fs::read(home.join("auth.json")).ok(),
        before_auth,
        "选择API/聚合请求目标或兼容登录字段不授权改写启动登录"
    );
    switch_official_account_in_home(
        &store,
        &home,
        ordinary.settings,
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"official-account"}}"#,
    )
    .unwrap();
    let live_config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    let live_auth: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("auth.json")).unwrap()).unwrap();

    assert!(live_config.contains(r#"model_provider = "openai""#));
    assert!(!live_config.contains(r#"base_url = "https://api.example/v1""#));
    assert!(!live_config.contains(r#"experimental_bearer_token = "sk-api""#));
    assert_eq!(live_auth["auth_mode"], "chatgpt");
    assert_eq!(live_auth["tokens"]["access_token"], "official-account");
}

#[test]
fn mixed_login_account_change_requires_explicit_authentication_api() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        home.join("auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"account-a"}}"#,
    )
    .unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let account_a = RelayProfile {
        id: "official-a".to_string(),
        name: "OpenAI A".to_string(),
        relay_mode: RelayMode::Official,
        auth_contents: r#"{"auth_mode":"chatgpt","tokens":{"access_token":"account-a"}}"#
            .to_string(),
        ..RelayProfile::default()
    };
    let account_b = RelayProfile {
        id: "official-b".to_string(),
        name: "OpenAI B".to_string(),
        relay_mode: RelayMode::Official,
        auth_contents: r#"{"auth_mode":"chatgpt","tokens":{"access_token":"account-b"}}"#
            .to_string(),
        ..RelayProfile::default()
    };
    let selected_auth = account_b.auth_contents.clone();
    let next = BackendSettings {
        active_relay_id: "official-a".to_string(),
        relay_profiles: vec![account_a, account_b],
        official_login_mixed_mode: true,
        official_login_relay_id: "official-b".to_string(),
        ..BackendSettings::default()
    };

    let before_auth = std::fs::read(home.join("auth.json")).ok();
    let ordinary = switch_relay_profile_in_home(&store, &home, next, "official-a").unwrap();
    assert_eq!(
        std::fs::read(home.join("auth.json")).ok(),
        before_auth,
        "选择API/聚合请求目标或兼容登录字段不授权改写启动登录"
    );
    switch_official_account_in_home(&store, &home, ordinary.settings, &selected_auth).unwrap();
    let live_auth: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("auth.json")).unwrap()).unwrap();

    assert_eq!(live_auth["tokens"]["access_token"], "account-b");
}

#[test]
fn explicit_official_account_switch_updates_auth_while_pure_api_remains_active() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        home.join("auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"account-a"}}"#,
    )
    .unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let next = BackendSettings {
        active_relay_id: "api".to_string(),
        active_official_account_id: "account-b".to_string(),
        relay_profiles: vec![pure_profile("api", "https://api.example/v1", "sk-api")],
        ..BackendSettings::default()
    };
    let target_auth = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"account-b"}}"#;

    switch_official_account_in_home(&store, &home, next, target_auth).unwrap();
    let live_config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    let live_auth = std::fs::read_to_string(home.join("auth.json")).unwrap();

    assert!(live_config.contains(r#"base_url = "https://api.example/v1""#));
    assert!(live_config.contains(r#"experimental_bearer_token = "sk-api""#));
    assert!(live_auth.contains("account-b"));
    assert_eq!(
        store.load().unwrap().active_official_account_id,
        "account-b"
    );
}

#[test]
fn explicit_official_account_switch_still_works_when_provider_switching_is_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    let original_config = "model_provider = \"untouched\"\n";
    std::fs::write(home.join("config.toml"), original_config).unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let next = BackendSettings {
        relay_profiles_enabled: false,
        active_official_account_id: "account-b".to_string(),
        ..BackendSettings::default()
    };
    let target_auth = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"account-b"}}"#;

    switch_official_account_in_home(&store, &home, next, target_auth).unwrap();

    assert_eq!(
        std::fs::read_to_string(home.join("config.toml")).unwrap(),
        original_config
    );
    assert!(
        std::fs::read_to_string(home.join("auth.json"))
            .unwrap()
            .contains("account-b")
    );
}

#[test]
fn switch_preserves_previous_official_profile_when_selecting_pure_api_provider() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        r#"model = "gpt-5.5"
model_reasoning_effort = "high"
model_provider = "custom"

[model_providers.custom]
name = "custom"
wire_api = "responses"
requires_openai_auth = true
base_url = "https://third-party.example/v1"

[features]
goals = true
"#,
    )
    .unwrap();
    std::fs::write(
        home.join("auth.json"),
        r#"{"OPENAI_API_KEY":"sk-third-party"}"#,
    )
    .unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let official = RelayProfile {
        id: "official".to_string(),
        name: "官方".to_string(),
        relay_mode: RelayMode::Official,
        official_mix_api_key: false,
        hide_official_usage_alert: false,
        auth_contents: r#"{"auth_mode":"chatgpt","tokens":{"access_token":"official"}}"#
            .to_string(),
        ..RelayProfile::default()
    };
    let pure = pure_profile("api", "https://third-party.example/v1", "sk-third-party");
    let original = BackendSettings {
        active_relay_id: "official".to_string(),
        relay_profiles: vec![official.clone(), pure.clone()],
        ..BackendSettings::default()
    };
    store.save(&original).unwrap();
    let next = BackendSettings {
        active_relay_id: "api".to_string(),
        relay_profiles: vec![official, pure],
        ..BackendSettings::default()
    };

    let result = switch_current_relay_profile(&store, &home, next, "official").unwrap();
    let returned = result
        .settings
        .relay_profiles
        .iter()
        .find(|profile| profile.id == "official")
        .unwrap();

    assert_eq!(returned.relay_mode, RelayMode::Official);
    assert!(!returned.official_mix_api_key);
    assert!(returned.config_contents.is_empty());
    assert!(returned.api_key.is_empty());
}

#[test]
fn switch_captures_safe_app_state_before_writing_provider_config() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        home.join(".codex-global-state.json"),
        serde_json::json!({
            "electron-saved-workspace-roots": ["C:/work/app"],
            "prompt-history": ["do-not-copy"],
            "electron-persisted-atom-state": {
                "default-service-tier": "priority",
                "provider-token-cache": "do-not-copy"
            }
        })
        .to_string(),
    )
    .unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let original = BackendSettings {
        active_relay_id: "a".to_string(),
        relay_profiles: vec![
            pure_profile("a", "https://a.example/v1", "sk-a"),
            pure_profile("b", "https://b.example/v1", "sk-b"),
        ],
        ..BackendSettings::default()
    };
    store.save(&original).unwrap();
    let next = BackendSettings {
        active_relay_id: "b".to_string(),
        relay_profiles: original.relay_profiles.clone(),
        ..BackendSettings::default()
    };

    switch_current_relay_profile(&store, &home, next, "a").unwrap();

    let snapshot: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            home.join("backups_state")
                .join("app-state-sync")
                .join("latest-safe-state.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        snapshot["state"]["electron-saved-workspace-roots"],
        serde_json::json!(["C:\\work\\app"])
    );
    assert_eq!(
        snapshot["state"]["electron-persisted-atom-state"]["default-service-tier"],
        "priority"
    );
    assert!(snapshot["state"].get("prompt-history").is_none());
    assert!(
        snapshot["state"]["electron-persisted-atom-state"]
            .get("provider-token-cache")
            .is_none()
    );
}

#[test]
fn switch_from_aggregate_preserves_profile_and_live_auth() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    let original_auth = r#"{"OPENAI_API_KEY":"sk-old-api"}"#;
    std::fs::write(home.join("auth.json"), original_auth).unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let api = pure_profile("api", "https://api.example/v1", "sk-api");
    let aggregate = aggregate_profile("agg");
    let original = BackendSettings {
        active_relay_id: "api".to_string(),
        relay_profiles: vec![api.clone(), aggregate.clone()],
        ..BackendSettings::default()
    };
    store.save(&original).unwrap();
    let mut aggregate_target = aggregate_target_settings(api.clone(), aggregate.clone());
    aggregate_target.settings_revision = store.load().unwrap().settings_revision;
    switch_relay_profile_in_home(&store, &home, aggregate_target, "api").unwrap();
    let mut restored = original;
    restored.settings_revision = store.load().unwrap().settings_revision;
    switch_relay_profile_in_home(&store, &home, restored, "agg").unwrap();
    let saved = store.load().unwrap();
    assert!(
        saved
            .relay_profiles
            .iter()
            .find(|profile| profile.id == "agg")
            .unwrap()
            .auth_contents
            .is_empty()
    );
    assert_eq!(
        std::fs::read_to_string(home.join("auth.json")).unwrap(),
        original_auth
    );
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(config.contains(r#"experimental_bearer_token = "sk-api""#));
}

#[test]
fn switch_to_aggregate_does_not_restore_auth_from_supplier_snapshot() {
    assert_aggregate_switch_preserves_auth(
        r#"{"OPENAI_API_KEY": "sk-broken""#,
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"snapshot-token"}}"#,
    );
}

#[test]
fn switch_to_aggregate_ignores_corrupt_profile_snapshot_when_live_is_valid() {
    assert_aggregate_switch_preserves_auth(r#"{"OPENAI_API_KEY":"sk-old-api"}"#, "{oops");
}

#[test]
fn switch_to_aggregate_keeps_fresh_live_tokens_over_stale_profile_snapshot() {
    assert_aggregate_switch_preserves_auth(
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"fresh-token"}}"#,
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"stale-token"}}"#,
    );
}

#[test]
fn switch_to_aggregate_preserves_corrupt_auth_without_using_supplier_snapshot() {
    assert_aggregate_switch_preserves_auth(r#"{"OPENAI_API_KEY": "sk-broken""#, "{oops");
}

/// 回归（issue #1604）：官方 OAuth 登录态在「官方 → 聚合 → 官方」往返后必须完好。
/// 聚合使用 provider bearer；切换前后 OAuth 文件必须保持独立。
#[test]
fn official_oauth_survives_aggregate_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        home.join("auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"access-token","refresh_token":"refresh-token"},"last_refresh":"2026-09-23T00:00:00Z"}"#,
    )
    .unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let official = RelayProfile {
        id: "official".to_string(),
        name: "官方".to_string(),
        relay_mode: RelayMode::Official,
        ..RelayProfile::default()
    };
    let aggregate = aggregate_profile("agg");
    let round_trip_settings = |active: &str| BackendSettings {
        active_relay_id: active.to_string(),
        relay_profiles: vec![
            official.clone(),
            pure_profile("api", "https://api.example/v1", "sk-api"),
            aggregate.clone(),
        ],
        aggregate_relay_profiles: vec![AggregateRelayProfile {
            id: "agg".to_string(),
            name: "聚合".to_string(),
            session_provider: RelaySessionProvider::Custom,
            model_aggregation_enabled: false,
            strategy: AggregateRelayStrategy::Failover,
            members: vec![AggregateRelayMember {
                relay_id: "api".to_string(),
                weight: 1,
            }],
            routes: Vec::new(),
            model_mappings_enabled: true,
            model_mappings: Vec::new(),
            default_relay_id: String::new(),
            sync_default_member_login: false,
            auto_review_relay_id: String::new(),
        }],
        active_aggregate_relay_id: "agg".to_string(),
        ..BackendSettings::default()
    };
    store.save(&round_trip_settings("official")).unwrap();

    // 官方 → 聚合
    switch_current_relay_profile(&store, &home, round_trip_settings("agg"), "official").unwrap();
    let after_switch: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("auth.json")).unwrap()).unwrap();
    assert_eq!(
        after_switch
            .get("tokens")
            .and_then(|tokens| tokens.get("access_token"))
            .and_then(|token| token.as_str()),
        Some("access-token"),
        "切到聚合不能清掉官方 OAuth token"
    );
    assert_eq!(
        after_switch
            .get("OPENAI_API_KEY")
            .and_then(|item| item.as_str()),
        None
    );

    // 聚合 → 官方
    switch_current_relay_profile(&store, &home, round_trip_settings("official"), "agg").unwrap();
    let final_auth: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("auth.json")).unwrap()).unwrap();
    assert_eq!(
        final_auth
            .get("tokens")
            .and_then(|tokens| tokens.get("access_token"))
            .and_then(|token| token.as_str()),
        Some("access-token"),
        "切回官方后 OAuth token 必须仍在"
    );
    assert!(
        final_auth.get("OPENAI_API_KEY").is_none(),
        "切回官方应清掉中转 key，实际：{final_auth}"
    );
}

fn aggregate_profile(id: &str) -> RelayProfile {
    RelayProfile {
        id: id.to_string(),
        name: "聚合供应商".to_string(),
        relay_mode: RelayMode::Aggregate,
        config_contents: String::new(),
        auth_contents: String::new(),
        ..RelayProfile::default()
    }
}

fn aggregate_profile_with_auth(id: &str, auth_contents: &str) -> RelayProfile {
    RelayProfile {
        auth_contents: auth_contents.to_string(),
        ..aggregate_profile(id)
    }
}

fn aggregate_target_settings(api: RelayProfile, aggregate: RelayProfile) -> BackendSettings {
    BackendSettings {
        active_relay_id: "agg".to_string(),
        relay_profiles: vec![api, aggregate],
        aggregate_relay_profiles: vec![AggregateRelayProfile {
            id: "agg".to_string(),
            name: "聚合供应商".to_string(),
            session_provider: RelaySessionProvider::Custom,
            model_aggregation_enabled: false,
            strategy: AggregateRelayStrategy::Failover,
            members: vec![AggregateRelayMember {
                relay_id: "api".to_string(),
                weight: 1,
            }],
            routes: Vec::new(),
            model_mappings_enabled: true,
            model_mappings: Vec::new(),
            default_relay_id: String::new(),
            sync_default_member_login: false,
            auto_review_relay_id: String::new(),
        }],
        active_aggregate_relay_id: "agg".to_string(),
        ..BackendSettings::default()
    }
}

fn switch_api_to_aggregate(home: &std::path::Path, temp: &tempfile::TempDir) -> anyhow::Result<()> {
    switch_api_to_aggregate_with(home, temp, aggregate_profile("agg"))
}

fn switch_api_to_aggregate_with(
    home: &std::path::Path,
    temp: &tempfile::TempDir,
    aggregate: RelayProfile,
) -> anyhow::Result<()> {
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let api = pure_profile("api", "https://api.example/v1", "sk-api");
    let original = BackendSettings {
        active_relay_id: "api".to_string(),
        relay_profiles: vec![api.clone(), aggregate.clone()],
        ..BackendSettings::default()
    };
    store.save(&original)?;
    switch_current_relay_profile(
        &store,
        home,
        aggregate_target_settings(api, aggregate),
        "api",
    )?;
    Ok(())
}

#[test]
fn switch_to_aggregate_keeps_api_key_usable() {
    assert_aggregate_switch_preserves_auth(r#"{"OPENAI_API_KEY":"sk-old-api"}"#, "");
}

#[test]
fn switch_to_aggregate_does_not_create_login_credentials_from_empty_auth() {
    assert_aggregate_switch_preserves_auth("", "");
}

#[test]
fn switch_to_aggregate_preserves_chatgpt_oauth_tokens() {
    assert_aggregate_switch_preserves_auth(
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"access-token","refresh_token":"refresh-token"},"last_refresh":"2026-09-23T00:00:00Z"}"#,
        "",
    );
}

#[test]
fn switch_to_aggregate_preserves_corrupt_auth_json() {
    assert_aggregate_switch_preserves_auth(r#"{"OPENAI_API_KEY": "sk-broken""#, "");
}

fn assert_aggregate_switch_preserves_auth(auth: &str, snapshot: &str) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("auth.json"), auth).unwrap();
    switch_api_to_aggregate_with(&home, &temp, aggregate_profile_with_auth("agg", snapshot))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(home.join("auth.json")).unwrap(),
        auth
    );
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(config.contains(r#"experimental_bearer_token = "codex-plus-aggregate""#));
    assert!(config.contains("requires_openai_auth = false"));
}

fn pure_profile(id: &str, base_url: &str, key: &str) -> RelayProfile {
    RelayProfile {
        id: id.to_string(),
        name: id.to_uppercase(),
        relay_mode: RelayMode::PureApi,
        config_contents: format!(
            r#"model_provider = "custom"

[model_providers.custom]
name = "custom"
wire_api = "responses"
requires_openai_auth = true
base_url = "{base_url}"
"#
        ),
        auth_contents: format!(r#"{{"OPENAI_API_KEY":"{key}"}}"#),
        ..RelayProfile::default()
    }
}

/// issue #1097 问题 2 端到端回归：被污染的 live `model =`（整段转义后的供应商
/// 配置）在真实切换链路（backfill → save → load+normalize → apply）中逐轮转义
/// 放大。修复前每轮反斜杠翻倍、settings.json 膨胀；修复后首轮即被清洗且有界。
#[test]
fn switch_cycles_do_not_amplify_polluted_model() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir_all(&home).unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));

    std::fs::write(
        home.join("config.toml"),
        r#"model = "gpt-5.6-sol\n\nmodel_provider = \"custom\"\nbase_url = \"https://a.example/v1\"\n"
model_provider = "custom"

[model_providers.custom]
name = "custom"
wire_api = "responses"
requires_openai_auth = true
base_url = "https://a.example/v1"
"#,
    )
    .unwrap();
    std::fs::write(home.join("auth.json"), r#"{"OPENAI_API_KEY":"sk-a"}"#).unwrap();

    let mut settings = BackendSettings {
        active_relay_id: "a".to_string(),
        relay_profiles: vec![
            pure_profile("a", "https://a.example/v1", "sk-a"),
            pure_profile("b", "https://b.example/v1", "sk-b"),
        ],
        ..BackendSettings::default()
    };
    store.save(&settings).unwrap();

    for cycle in 0..10 {
        let previous = settings.active_relay_id.clone();
        settings.active_relay_id = if cycle % 2 == 0 { "b" } else { "a" }.to_string();
        let result =
            switch_current_relay_profile(&store, &home, settings.clone(), &previous).unwrap();
        settings = result.settings;
    }

    let final_config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    let final_settings_len = std::fs::read(temp.path().join("settings.json"))
        .unwrap()
        .len();
    assert!(
        !final_config.contains(r"\nmodel_provider"),
        "污染的 model 值残留于 live config:\n{final_config}"
    );
    assert!(
        final_settings_len < 20_000,
        "settings.json 经 10 轮切换膨胀到 {final_settings_len} 字节"
    );
    // 切换链路本身未被破坏：live config 仍是最后一个激活供应商（a）的合法配置
    assert!(final_config.contains(r#"base_url = "https://a.example/v1""#));
}

/// 回归（issue #1888）：上一个供应商已不在配置列表里时，错误里必须带上它的 id，
/// 用户才知道是哪一个没了（否则只看到「回填当前供应商配置失败」这一句无从下手）。
#[test]
fn switch_reports_previous_profile_id_when_it_is_missing() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    store
        .save(&BackendSettings {
            active_relay_id: "a".to_string(),
            relay_profiles: vec![pure_profile("a", "https://a.example/v1", "sk-a")],
            ..BackendSettings::default()
        })
        .unwrap();
    // 目标设置里只剩 b，上一个活跃的 a 已被删掉。
    let next = BackendSettings {
        active_relay_id: "b".to_string(),
        relay_profiles: vec![
            pure_profile("a", "https://a.example/v1", "sk-a"),
            pure_profile("b", "https://b.example/v1", "sk-b"),
        ],
        ..BackendSettings::default()
    };

    let error = switch_current_relay_profile(&store, &home, next, "deleted-id")
        .expect_err("上一个供应商不存在时必须中止切换");
    let message = error.to_string();

    assert!(
        message.contains("deleted-id"),
        "错误信息要带上缺失的供应商 id，实际：{message}"
    );
    assert!(
        message.contains("已不在配置列表中"),
        "错误信息要说明原因，实际：{message}"
    );
    // 中止切换后不能覆盖用户的磁盘配置。
    assert_eq!(store.load().unwrap().active_relay_id, "a");
    assert!(!home.join("config.toml").exists());
}

/// 回归：列表页按分页过滤后提交的子集设置（只带当前分页的聚合、成员供应商全丢）
/// 不能丢配置，也不能因成员不在请求里而拒绝切换。
#[test]
fn switch_restores_profiles_filtered_out_of_request() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::new(temp.path().join("settings.json"));
    let home = temp.path().join("codex");
    std::fs::create_dir_all(&home).unwrap();

    let aggregate_profile = RelayProfile {
        id: "agg".to_string(),
        name: "聚合".to_string(),
        relay_mode: RelayMode::Aggregate,
        ..RelayProfile::default()
    };
    let original = BackendSettings {
        active_relay_id: "deepseek".to_string(),
        relay_profiles: vec![
            pure_profile("deepseek", "https://deepseek.example/v1", "sk-deepseek"),
            pure_profile("openai", "https://openai.example/v1", "sk-openai"),
            aggregate_profile.clone(),
        ],
        aggregate_relay_profiles: vec![AggregateRelayProfile {
            id: "agg".to_string(),
            name: "聚合".to_string(),
            session_provider: RelaySessionProvider::Custom,
            model_aggregation_enabled: true,
            strategy: AggregateRelayStrategy::Failover,
            members: vec![
                AggregateRelayMember {
                    relay_id: "deepseek".to_string(),
                    weight: 1,
                },
                AggregateRelayMember {
                    relay_id: "openai".to_string(),
                    weight: 1,
                },
            ],
            routes: Vec::new(),
            default_relay_id: "deepseek".to_string(),
            sync_default_member_login: false,
            model_mappings_enabled: true,
            model_mappings: Vec::new(),
            auto_review_relay_id: String::new(),
        }],
        ..BackendSettings::default()
    };
    store.save(&original).unwrap();

    let filtered = BackendSettings {
        active_relay_id: "agg".to_string(),
        relay_profiles: vec![aggregate_profile],
        ..BackendSettings::default()
    };
    let result = switch_current_relay_profile(&store, &home, filtered, "deepseek").unwrap();
    assert_eq!(result.settings.active_relay_id, "agg");

    let saved = store.load().unwrap();
    let ids: Vec<String> = saved
        .relay_profiles
        .iter()
        .map(|profile| profile.id.clone())
        .collect();
    assert!(ids.contains(&"deepseek".to_string()), "{ids:?}");
    assert!(ids.contains(&"openai".to_string()), "{ids:?}");
    assert_eq!(
        saved.aggregate_relay_profiles.len(),
        1,
        "聚合成员配置不能被过滤请求吞掉"
    );
    assert_eq!(
        saved.aggregate_relay_profiles[0].default_relay_id, "deepseek",
        "默认供应商配置要跟着聚合一起回填"
    );
}

// 这些测试验证切换、回填和回滚；请求携带当前版本，避免先被并发保护拒绝。
fn switch_current_relay_profile(
    store: &SettingsStore,
    home: &std::path::Path,
    mut settings: BackendSettings,
    previous_active_relay_id: &str,
) -> anyhow::Result<codex_plus_core::relay_switch::RelaySwitchResult> {
    settings.settings_revision = store.revision()?;
    switch_relay_profile_in_home(store, home, settings, previous_active_relay_id)
}

/// issue #2601：live auth.json 是 Codex 自己写的 API Key 登录态
/// （`auth_mode = "apikey"`），当前供应商是内置的「默认中转」（官方登录、非混合）。
/// 切换前回填过去会把这份 API Key 认证原样吸进官方快照，随后存储校验判为
/// 「不支持的官方认证存档格式」，整次切换失败、config.toml 不再写入。
#[test]
fn switch_from_default_official_ignores_live_api_key_auth() {
    // 诊断日志路径是进程级状态，与其它测试模块串行。
    let _state = crate::common::global_state_lock();
    for saved_auth in [
        String::new(),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"official"}}"#.to_string(),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(
            home.join("config.toml"),
            "model = \"gpt-5.5\"\nmodel_reasoning_effort = \"high\"\n",
        )
        .unwrap();
        std::fs::write(
            home.join("auth.json"),
            r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-test"}"#,
        )
        .unwrap();
        let log_path = temp.path().join("diagnostic.log");
        codex_plus_core::diagnostic_log::set_diagnostic_log_path_for_tests(Some(log_path.clone()));
        let store = SettingsStore::new(temp.path().join("settings.json"));
        let official = RelayProfile {
            auth_contents: saved_auth.clone(),
            ..RelayProfile::default()
        };
        let pure = pure_profile("deepseek", "https://api.deepseek.example/v1", "sk-deepseek");
        let original = BackendSettings {
            active_relay_id: "default".to_string(),
            relay_profiles: vec![official.clone(), pure.clone()],
            ..BackendSettings::default()
        };
        store.save(&original).unwrap();
        let next = BackendSettings {
            active_relay_id: "deepseek".to_string(),
            relay_profiles: vec![official, pure],
            ..BackendSettings::default()
        };

        let result = switch_current_relay_profile(&store, &home, next, "default");
        codex_plus_core::diagnostic_log::set_diagnostic_log_path_for_tests(None);
        let result = result
            .unwrap_or_else(|error| panic!("切换不应失败（saved_auth={saved_auth}）：{error:#}"));
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        assert!(
            log.contains("relay_backfill.official_auth.kept_snapshot") && !log.contains("sk-test"),
            "应记一条不含密钥的诊断日志：{log}"
        );

        let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(
            config.contains("https://api.deepseek.example/v1"),
            "目标供应商应写入 config.toml：{config}"
        );
        let stored = store.load().unwrap();
        assert_eq!(stored.active_relay_id, "deepseek");
        for settings in [&result.settings, &stored] {
            let default = settings
                .relay_profiles
                .iter()
                .find(|profile| profile.id == "default")
                .unwrap();
            assert!(
                !default.auth_contents.contains("sk-test")
                    && !default.auth_contents.contains("apikey"),
                "官方快照不应吸收 live 的 API Key 身份：{}",
                default.auth_contents
            );
            if !saved_auth.is_empty() {
                assert!(
                    default.auth_contents.contains("\"access_token\""),
                    "原有官方登录快照应保留：{}",
                    default.auth_contents
                );
            }
        }
    }
}
