use codex_plus_core::relay_config::{
    apply_relay_profile_to_home_with_switch_rules, backfill_relay_profile_from_home_with_common,
    normalize_relay_profile_for_storage,
};
use codex_plus_core::settings::{RelayMode, RelayModelRoute, RelayProfile};
use serde_json::{Value, json};

fn provider_config(flag: Option<bool>) -> String {
    let auth = flag
        .map(|value| format!("requires_openai_auth = {value}\n"))
        .unwrap_or_default();
    format!(
        "model_provider = \"vendor\"\n\n[model_providers.vendor]\nname = \"Vendor\"\nwire_api = \"responses\"\nbase_url = \"https://relay.example/v1\"\n{auth}"
    )
}

fn pure_profile(flag: Option<bool>) -> RelayProfile {
    RelayProfile {
        relay_mode: RelayMode::PureApi,
        config_contents: provider_config(flag),
        auth_contents: json!({"OPENAI_API_KEY": "fixture-provider-key"}).to_string(),
        ..RelayProfile::default()
    }
}

#[test]
fn pure_api_switch_uses_provider_auth_and_preserves_the_independent_login_source() {
    let live_oauth = b"{ \"auth_mode\": \"chatgpt\", \"tokens\":{\"access_token\":\"fixture-oauth\"}, \"unknown\": 7 }\r\n";
    for flag in [None, Some(false), Some(true)] {
        for live in [None, Some(live_oauth.as_slice())] {
            let temp = tempfile::tempdir().unwrap();
            if let Some(contents) = live {
                std::fs::write(temp.path().join("auth.json"), contents).unwrap();
            }
            let mut profile = pure_profile(flag);
            normalize_relay_profile_for_storage(&mut profile).unwrap();
            assert!(
                profile
                    .config_contents
                    .contains(r#"experimental_bearer_token = "fixture-provider-key""#)
            );
            apply_relay_profile_to_home_with_switch_rules(temp.path(), &profile, "").unwrap();
            let config = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
            assert!(config.contains(r#"experimental_bearer_token = "fixture-provider-key""#));
            // API 目标不能因旧快照中的 requires_openai_auth=true 而借用官方登录。
            let parsed: toml::Value = config.parse().unwrap();
            assert_eq!(
                parsed["model_providers"]["custom"]["requires_openai_auth"].as_bool(),
                Some(false)
            );
            match live {
                Some(contents) => assert_eq!(
                    std::fs::read(temp.path().join("auth.json")).unwrap(),
                    contents
                ),
                None => assert!(!temp.path().join("auth.json").exists()),
            }
            assert!(!config.contains("fixture-oauth"));
            assert!(!profile.auth_contents.contains("fixture-oauth"));
        }
    }
}

#[test]
fn pure_api_inline_import_keeps_selected_provider_auth_without_creating_live_auth() {
    let temp = tempfile::tempdir().unwrap();
    let mut profile = RelayProfile {
        relay_mode: RelayMode::PureApi,
        config_contents: format!(
            "{}experimental_bearer_token = \"fixture-import-key\"\n",
            provider_config(Some(false))
        ),
        ..RelayProfile::default()
    };
    normalize_relay_profile_for_storage(&mut profile).unwrap();
    for _ in 0..2 {
        apply_relay_profile_to_home_with_switch_rules(temp.path(), &profile, "").unwrap();
        let config = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
        assert!(config.contains(r#"experimental_bearer_token = "fixture-import-key""#));
        assert!(config.contains("requires_openai_auth = false"));
        assert_eq!(
            codex_plus_core::relay_config::relay_profile_api_key(&profile),
            "fixture-import-key"
        );
        assert_eq!(
            profile.selected_api_key().unwrap().api_key,
            "fixture-import-key"
        );
        assert!(profile.auth_contents.is_empty());
        assert!(!temp.path().join("auth.json").exists());
        normalize_relay_profile_for_storage(&mut profile).unwrap();
    }
}

#[test]
fn pure_api_backfill_keeps_archived_provider_auth_instead_of_live_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let mut profile = pure_profile(Some(false));
    std::fs::write(
        temp.path().join("config.toml"),
        format!(
            "{}experimental_bearer_token = \"fixture-unrelated-live-key\"\n",
            provider_config(Some(false))
        ),
    )
    .unwrap();
    std::fs::write(
        temp.path().join("auth.json"),
        json!({"OPENAI_API_KEY": "fixture-unrelated-live-key", "tokens": {"access_token": "fixture-oauth"}})
            .to_string(),
    )
    .unwrap();
    let live_auth = std::fs::read(temp.path().join("auth.json")).unwrap();
    backfill_relay_profile_from_home_with_common(temp.path(), &mut profile, &mut String::new())
        .unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("auth.json")).unwrap(),
        live_auth
    );
    assert!(
        profile
            .config_contents
            .contains(r#"experimental_bearer_token = "fixture-provider-key""#)
    );
    assert!(
        !profile
            .config_contents
            .contains("fixture-unrelated-live-key")
    );
    assert!(!profile.auth_contents.contains("fixture-oauth"));
    apply_relay_profile_to_home_with_switch_rules(temp.path(), &profile, "").unwrap();
    let config = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(config.contains(r#"experimental_bearer_token = "fixture-provider-key""#));
    assert_eq!(
        std::fs::read(temp.path().join("auth.json")).unwrap(),
        live_auth
    );
}

#[test]
fn pure_api_model_routes_keep_explicit_auth_for_local_transport() {
    let temp = tempfile::tempdir().unwrap();
    let mut profile = pure_profile(Some(false));
    profile.model_routes.push(RelayModelRoute {
        model: "visible-model".to_string(),
        target_relay_id: "upstream-profile".to_string(),
        target_model: "upstream-model".to_string(),
    });
    normalize_relay_profile_for_storage(&mut profile).unwrap();
    apply_relay_profile_to_home_with_switch_rules(temp.path(), &profile, "").unwrap();
    let config = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(config.contains(&format!(
        "base_url = \"{}\"",
        codex_plus_core::protocol_proxy::local_responses_proxy_base_url(
            codex_plus_core::protocol_proxy::protocol_proxy_port()
        )
    )));
    assert!(config.contains(r#"experimental_bearer_token = "fixture-provider-key""#));
    assert!(config.contains("requires_openai_auth = false"));
    assert!(!temp.path().join("auth.json").exists());
}

#[test]
fn no_auth_keeps_local_placeholder_without_touching_an_independent_login() {
    let live_oauth = b"{ \"tokens\": {\"access_token\":\"keep-live-login\"}, \"unknown\": 9 }\r\n";
    for live in [None, Some(live_oauth.as_slice())] {
        let temp = tempfile::tempdir().unwrap();
        if let Some(contents) = live {
            std::fs::write(temp.path().join("auth.json"), contents).unwrap();
        }
        let mut profile = pure_profile(Some(false));
        profile.no_auth = true;
        normalize_relay_profile_for_storage(&mut profile).unwrap();
        apply_relay_profile_to_home_with_switch_rules(temp.path(), &profile, "").unwrap();
        let config = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
        assert!(config.contains(r#"experimental_bearer_token = "codex-plus-no-auth""#));
        assert!(!config.contains("fixture-provider-key"));
        assert!(!config.contains("keep-live-login"));
        let auth: Value = serde_json::from_str(&profile.auth_contents).unwrap();
        assert!(auth.get("OPENAI_API_KEY").is_none());
        assert!(profile.api_key.is_empty());
        backfill_relay_profile_from_home_with_common(temp.path(), &mut profile, &mut String::new())
            .unwrap();
        assert!(
            profile
                .config_contents
                .contains(r#"experimental_bearer_token = "codex-plus-no-auth""#)
        );
        assert!(!profile.auth_contents.contains("keep-live-login"));
        match live {
            Some(contents) => assert_eq!(
                std::fs::read(temp.path().join("auth.json")).unwrap(),
                contents
            ),
            None => assert!(!temp.path().join("auth.json").exists()),
        }
    }
}
