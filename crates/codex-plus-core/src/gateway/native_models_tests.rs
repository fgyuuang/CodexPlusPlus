use super::*;
use crate::gateway::apply::codex_gateway_model_catalog;
use crate::settings::{RelayMode, RelayProtocol};
use serde_json::{Value, json};

fn provider(id: &str, account: Option<&str>) -> GatewayProvider {
    serde_json::from_value(json!({"relay": {
        "id": id, "name": id, "modelList": "future-native\napi-only",
        "baseUrl": "https://example.invalid/v1", "apiKey": "test-key",
        "authContents": account.map(|account| json!({
            "codex_plus_official_account_ref": account, "codex_plus_gateway": true
        }).to_string()).unwrap_or_default()
    }}))
    .unwrap()
}

fn fixture() -> GatewaySettings {
    let mut gateway = GatewaySettings::default();
    gateway.providers = vec![
        provider("supplier", None),
        provider("login", Some("account-a")),
        provider("other", Some("account-b")),
    ];
    gateway.bindings.insert("codex".into(), AgentBinding {
        enabled: true,
        model: "login/future-native".into(),
        native_official: Some(RelayProfile {
            id: "login".into(), name: "当前登录账号".into(),
            relay_mode: RelayMode::Official, protocol: RelayProtocol::Responses,
            model: "future-native".into(), model_list: "future-native\nnative-only".into(),
            auth_contents: json!({"codex_plus_official_account_ref": "account-a", "codex_plus_gateway": true}).to_string(),
            ..RelayProfile::default()
        }),
        ..AgentBinding::default()
    });
    gateway
}

#[test]
fn native_catalog_deduplicates_login_and_keeps_sources_and_automatic_entry() {
    let gateway = fixture();
    let binding = gateway.binding(&ToolId::Codex).unwrap();
    let catalog = codex_gateway_model_catalog(&gateway, binding);
    let models: Vec<_> = catalog.model_list.lines().collect();
    assert!(models.contains(&"future-native"));
    assert!(models.contains(&"native-only"));
    assert!(!models.contains(&"login/future-native"));
    assert!(models.contains(&"supplier/future-native"));
    assert!(models.contains(&"other/future-native"));
    assert!(models.contains(&"codex-plus-auto/future-native"));
    assert_eq!(binding_default_model(&gateway, binding), "future-native");
    let metadata: Value = serde_json::from_str(&catalog.model_metadata).unwrap();
    assert_eq!(
        metadata["future-native"]["codex_plus_group"],
        "当前登录账号"
    );
    assert!(
        !metadata["future-native"]["display_name"]
            .as_str()
            .unwrap()
            .contains('（')
    );
    assert_eq!(
        metadata["codex-plus-auto/future-native"]["codex_plus_group"],
        "自动组"
    );
}

#[test]
fn native_route_is_single_account_and_historical_qualified_route_still_works() {
    let gateway = fixture();
    let binding = gateway.binding(&ToolId::Codex).unwrap();
    let bare = model_entry_members(&gateway, binding, "future-native");
    assert_eq!(bare.len(), 1);
    assert_eq!(bare[0].0.id, "login");
    let plan = preview_plan(&gateway, &ToolId::Codex, "future-native", None).unwrap();
    assert_eq!(plan.candidates.len(), 1);
    assert_eq!(plan.candidates[0].relay.id, "login");
    for (model, source) in [
        ("login/future-native", "login"),
        ("other/future-native", "other"),
        ("supplier/future-native", "supplier"),
    ] {
        let members = model_entry_members(&gateway, binding, model);
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].0.id, source);
    }
    assert_eq!(
        model_entry_members(&gateway, binding, "codex-plus-auto/future-native").len(),
        3
    );
}

#[test]
fn native_and_automatic_entries_keep_separate_upstream_compaction_settings() {
    let mut gateway = fixture();
    gateway.providers[0].auto_compact.fallback = Some(55.0);
    gateway.providers[1].auto_compact.fallback = Some(85.0);
    let binding = gateway.binding(&ToolId::Codex).unwrap();
    assert_eq!(
        model_entry_auto_compact(&gateway, binding, "future-native"),
        Some(85.0)
    );
    assert_eq!(
        model_entry_auto_compact(&gateway, binding, "codex-plus-auto/future-native"),
        Some(55.0)
    );
    gateway.providers.remove(1);
    let binding = gateway.binding(&ToolId::Codex).unwrap();
    assert_eq!(
        model_entry_auto_compact(&gateway, binding, "future-native"),
        None
    );
}

#[test]
fn disabling_automatic_switching_never_maps_its_entry_to_default() {
    let mut gateway = fixture();
    gateway.auto_models = false;
    let binding = gateway.binding(&ToolId::Codex).unwrap();
    assert!(
        !codex_binding_models(&gateway, binding)
            .iter()
            .any(|model| model == "codex-plus-auto/future-native")
    );
    assert!(
        preview_plan(
            &gateway,
            &ToolId::Codex,
            "codex-plus-auto/future-native",
            None
        )
        .is_err()
    );
    assert_eq!(
        model_entry_members(&gateway, binding, "future-native").len(),
        1
    );
    assert!(
        preview_plan(
            &gateway,
            &ToolId::Codex,
            "codex-plus-auto/future-native[1M]",
            None
        )
        .is_err()
    );
}

#[test]
fn projection_is_not_persisted_or_applied_to_other_agents() {
    let mut gateway = fixture();
    gateway.bindings.insert(
        "claude".into(),
        AgentBinding {
            enabled: true,
            ..AgentBinding::default()
        },
    );
    let binding = gateway.binding(&ToolId::Claude).unwrap();
    assert_eq!(
        model_entry_members(&gateway, binding, "future-native").len(),
        3
    );
    let saved = serde_json::to_value(&gateway).unwrap();
    assert!(saved["bindings"]["codex"].get("nativeOfficial").is_none());
    let restored: GatewaySettings = serde_json::from_value(saved).unwrap();
    assert!(
        restored
            .binding(&ToolId::Codex)
            .unwrap()
            .native_official
            .is_none()
    );
}

#[test]
fn native_model_without_a_pool_account_does_not_invent_an_automatic_group() {
    let mut gateway = fixture();
    gateway.providers.truncate(1);
    let binding = gateway.binding(&ToolId::Codex).unwrap();
    let models = codex_binding_models(&gateway, binding);
    assert!(models.iter().any(|model| model == "future-native"));
    assert!(
        !models
            .iter()
            .any(|model| model == "codex-plus-auto/future-native")
    );
    assert_eq!(
        model_entry_members(&gateway, binding, "future-native").len(),
        1
    );
}

#[test]
fn native_projection_tracks_login_instead_of_scheduling_primary() {
    let mut settings = crate::settings::BackendSettings::default();
    settings.official_experience.enabled = true;
    settings.active_official_account_id = "account-a".into();
    settings.official_experience.primary_official_account_id = "account-b".into();
    let fresh = GatewaySettings::default().with_native_codex_account(&settings);
    let fresh_binding = fresh.binding(&ToolId::Codex).unwrap();
    assert!(!fresh_binding.enabled, "仅展示当前账号目录，不擅自接入网关");
    assert!(
        fresh_binding.native_official.is_some(),
        "首次接入前也能选择原生模型"
    );
    let projected = fixture().with_native_codex_account(&settings);
    let native = projected
        .binding(&ToolId::Codex)
        .unwrap()
        .native_official
        .as_ref()
        .unwrap();
    assert_eq!(
        crate::gateway::official_accounts::account_ref(native).as_deref(),
        Some("account-a")
    );
    assert!(!native.model_list.is_empty());
    settings.active_official_account_id = "account-b".into();
    let projected = projected.with_native_codex_account(&settings);
    let native = projected
        .binding(&ToolId::Codex)
        .unwrap()
        .native_official
        .as_ref()
        .unwrap();
    assert_eq!(native.id, "other");
    assert_eq!(
        crate::gateway::official_accounts::account_ref(native).as_deref(),
        Some("account-b")
    );
    settings.active_official_account_id.clear();
    let projected = projected.with_native_codex_account(&settings);
    assert!(
        projected
            .binding(&ToolId::Codex)
            .unwrap()
            .native_official
            .is_none()
    );
}
