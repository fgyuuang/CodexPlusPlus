//! `syncDefaultMemberLogin` 仅为上游配置兼容字段，不是本地认证操作。
//!
//! 本地切换请求来源只写 config；官方认证必须由显式账号选择路径处理，参见
//! official_experience.rs。编辑器不再展示未生效的同步开关，存量字段仍可往返保存。
//! 这里覆盖存量开关、独立官方快照、live 登录及代理配置，禁止把聚合切换伪装成登录。

use base64::Engine;
use codex_plus_core::paths::set_settings_path_for_tests;
use codex_plus_core::relay_config::apply_relay_profile_to_home_with_switch_rules;
use codex_plus_core::settings::{
    AggregateRelayMember, AggregateRelayProfile, AggregateRelayStrategy, BackendSettings,
    RelayMode, RelayProfile, RelaySessionProvider, SettingsStore,
};
use serde_json::{Value, json};

const OFFICIAL_ID: &str = "official-account";
const API_ID: &str = "api-member";
const AGGREGATE_ID: &str = "agg";
const PROXY_KEY: &str = "codex-plus-aggregate";
static SETTINGS_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn account_auth(account: &str, generation: &str) -> String {
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        json!({
            "exp": 4_102_444_800_i64,
            "generation": generation,
            "https://api.openai.com/auth": { "chatgpt_account_id": account },
        })
        .to_string(),
    );
    let token = format!("{header}.{claims}.fixture");
    serde_json::to_string_pretty(&json!({
        "auth_mode": "chatgpt", "last_refresh": generation,
        "tokens": { "access_token": token, "refresh_token": format!("refresh-{generation}"),
            "id_token": token, "account_id": account },
        "preserved_unknown": { "generation": generation },
    }))
    .unwrap()
        + "\r\n"
}

fn aggregate_profile(sync: bool, default_member: &str) -> AggregateRelayProfile {
    AggregateRelayProfile {
        id: AGGREGATE_ID.to_string(),
        name: "聚合供应商".to_string(),
        session_provider: RelaySessionProvider::Custom,
        model_aggregation_enabled: true,
        strategy: AggregateRelayStrategy::Failover,
        members: vec![AggregateRelayMember {
            relay_id: API_ID.to_string(),
            weight: 1,
        }],
        routes: Vec::new(),
        default_relay_id: default_member.to_string(),
        sync_default_member_login: sync,
        model_mappings_enabled: true,
        model_mappings: Vec::new(),
    }
}

struct Applied {
    _dir: tempfile::TempDir,
    auth: Option<Vec<u8>>,
    config: toml::Value,
}

/// 官方快照是独立认证源；实际聚合只含 API 成员，不能把官方账号加入轮转。
fn apply_switch(snapshot: &str, sync: bool, default_member: &str, live: Option<&[u8]>) -> Applied {
    let _serial = SETTINGS_GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    if let Some(contents) = live {
        std::fs::write(home.join("auth.json"), contents).unwrap();
    }
    let profile = RelayProfile {
        id: AGGREGATE_ID.to_string(),
        name: "聚合供应商".to_string(),
        relay_mode: RelayMode::Aggregate,
        api_key: PROXY_KEY.to_string(),
        model: "cpx-api-member/vendor-model".to_string(),
        ..RelayProfile::default()
    };
    let settings = BackendSettings {
        official_login_relay_id: String::new(),
        active_relay_id: AGGREGATE_ID.to_string(),
        active_aggregate_relay_id: AGGREGATE_ID.to_string(),
        relay_profiles: vec![
            RelayProfile {
                id: OFFICIAL_ID.to_string(),
                relay_mode: RelayMode::Official,
                auth_contents: snapshot.to_string(),
                ..RelayProfile::default()
            },
            RelayProfile {
                id: API_ID.to_string(),
                name: "API".to_string(),
                relay_mode: RelayMode::PureApi,
                base_url: "https://api.example.invalid/v1".to_string(),
                api_key: "member-api-key".to_string(),
                model: "vendor-model".to_string(),
                model_list: "vendor-model".to_string(),
                config_contents: r#"model = "vendor-model"
model_provider = "custom"

[model_providers.custom]
name = "custom"
wire_api = "responses"
requires_openai_auth = true
base_url = "https://api.example.invalid/v1"
"#
                .to_string(),
                auth_contents: r#"{"OPENAI_API_KEY":"member-api-key"}"#.to_string(),
                ..RelayProfile::default()
            },
            profile.clone(),
        ],
        aggregate_relay_profiles: vec![aggregate_profile(sync, default_member)],
        ..BackendSettings::default()
    };
    let settings_path = dir.path().join("settings.json");
    let _guard = set_settings_path_for_tests(Some(settings_path.clone()));
    std::fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let applied = apply_relay_profile_to_home_with_switch_rules(&home, &profile, "").unwrap();
    assert!(
        applied.configured,
        "the API aggregate must remain configured without writing auth"
    );
    let persisted: BackendSettings =
        serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
    assert_eq!(
        persisted.aggregate_relay_profiles[0].sync_default_member_login,
        sync
    );
    assert_eq!(
        persisted.relay_profiles[0].auth_contents, snapshot,
        "source changes cannot update official snapshots"
    );
    let auth = match std::fs::read(home.join("auth.json")) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("read fixture auth: {error}"),
    };
    assert_eq!(
        auth.as_deref(),
        live,
        "source switching must preserve every live auth byte, including absence"
    );
    let config = std::fs::read_to_string(home.join("config.toml"))
        .unwrap()
        .parse()
        .unwrap();
    Applied {
        _dir: dir,
        auth,
        config,
    }
}

#[test]
fn compatibility_flag_round_trips_without_authorizing_auth_writes() {
    for sync in [false, true] {
        let aggregate = aggregate_profile(sync, API_ID);
        let raw = serde_json::to_value(&aggregate).unwrap();
        assert_eq!(raw["syncDefaultMemberLogin"], sync);
        let restored: AggregateRelayProfile = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(restored.sync_default_member_login, sync);
        let mut without_flag = raw;
        without_flag
            .as_object_mut()
            .unwrap()
            .remove("syncDefaultMemberLogin");
        assert!(
            !serde_json::from_value::<AggregateRelayProfile>(without_flag)
                .unwrap()
                .sync_default_member_login
        );
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(dir.path().join("settings.json"));
        store
            .save(&BackendSettings {
                aggregate_relay_profiles: vec![aggregate],
                ..BackendSettings::default()
            })
            .unwrap();
        let updated = store
            .update(json!({ "codexAppFastStartup": true }))
            .unwrap();
        assert_eq!(
            updated.aggregate_relay_profiles[0].sync_default_member_login,
            sync
        );
    }
}

#[test]
fn official_access_and_refresh_snapshots_never_create_live_auth() {
    let snapshots = [
        account_auth("acct-1", "snapshot"),
        json!({"tokens": {"access_token": "access-only", "account_id": "acct-1"}}).to_string(),
        json!({"tokens": {"refresh_token": "refresh-only", "account_id": "acct-1"}}).to_string(),
    ];
    for sync in [false, true] {
        for snapshot in &snapshots {
            assert!(apply_switch(snapshot, sync, API_ID, None).auth.is_none());
        }
    }
}

#[test]
fn same_account_access_and_refresh_generations_remain_byte_identical() {
    for sync in [false, true] {
        for (saved, live) in [("old", "new"), ("new", "old")] {
            let live = account_auth("acct-1", live);
            apply_switch(
                &account_auth("acct-1", saved),
                sync,
                API_ID,
                Some(live.as_bytes()),
            );
        }
    }
}

#[test]
fn another_live_official_account_cannot_be_replaced_by_a_snapshot() {
    let live = account_auth("acct-other", "live-other");
    for sync in [false, true] {
        apply_switch(
            &account_auth("acct-1", "snapshot"),
            sync,
            API_ID,
            Some(live.as_bytes()),
        );
    }
}

#[test]
fn existing_api_auth_is_not_replaced_with_the_aggregate_proxy_key() {
    let live = b"{  \"OPENAI_API_KEY\" : \"existing-user-key\", \"unknown\" : 7 }\r\n";
    for sync in [false, true] {
        let applied = apply_switch(
            &account_auth("acct-1", "snapshot"),
            sync,
            API_ID,
            Some(live),
        );
        let value: Value = serde_json::from_slice(applied.auth.as_ref().unwrap()).unwrap();
        assert_eq!(value["OPENAI_API_KEY"], "existing-user-key");
        assert!(value.get("tokens").is_none());
    }
}

#[test]
fn opaque_live_auth_is_not_reformatted_or_repaired_during_source_switches() {
    for live in [
        b"{\"unexpected\":true}\r\n".as_slice(),
        b"incomplete-auth-fixture\0\n".as_slice(),
    ] {
        for sync in [false, true] {
            apply_switch(
                &account_auth("acct-1", "snapshot"),
                sync,
                API_ID,
                Some(live),
            );
        }
    }
}

#[test]
fn missing_or_invalid_official_snapshots_cannot_clear_live_login() {
    let live = account_auth("acct-live", "live");
    for snapshot in ["", "{}", "not-json", r#"{"tokens":{"refresh_token":""}}"#] {
        for sync in [false, true] {
            apply_switch(snapshot, sync, API_ID, Some(live.as_bytes()));
        }
    }
}

#[test]
fn legacy_official_default_references_do_not_select_an_auth_source() {
    let live = account_auth("acct-live", "live");
    for default_member in [OFFICIAL_ID, "missing-member", ""] {
        for sync in [false, true] {
            apply_switch(
                &account_auth("acct-1", "snapshot"),
                sync,
                default_member,
                Some(live.as_bytes()),
            );
            assert!(
                apply_switch(
                    &account_auth("acct-1", "snapshot"),
                    sync,
                    default_member,
                    None
                )
                .auth
                .is_none()
            );
        }
    }
}

#[test]
fn proxy_transport_stays_separate_from_official_login_for_both_compatibility_values() {
    for sync in [false, true] {
        let applied = apply_switch(&account_auth("acct-1", "snapshot"), sync, API_ID, None);
        assert_eq!(applied.config["model_provider"].as_str(), Some("custom"));
        let provider = &applied.config["model_providers"]["custom"];
        assert_eq!(provider["requires_openai_auth"].as_bool(), Some(false));
        assert_eq!(
            provider["experimental_bearer_token"].as_str(),
            Some(PROXY_KEY)
        );
        assert_eq!(
            provider["base_url"].as_str(),
            Some(
                format!(
                    "http://127.0.0.1:{}/v1",
                    codex_plus_core::protocol_proxy::protocol_proxy_port()
                )
                .as_str()
            )
        );
        let text = applied.config.to_string();
        assert!(!text.contains("refresh-snapshot"));
        assert!(!text.contains("member-api-key"));
        assert!(applied.auth.is_none());
    }
}
