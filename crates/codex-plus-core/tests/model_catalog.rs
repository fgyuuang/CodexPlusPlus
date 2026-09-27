use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::thread;

use codex_plus_core::model_catalog::{
    read_codex_model_catalog, read_codex_model_catalog_from_home,
};
use codex_plus_core::settings::{
    BackendSettings, RelayMode, RelayProfile, RelayProtocol, SettingsStore,
};
use serde_json::json;

#[tokio::test]
async fn model_catalog_fetches_models_from_codex_config_provider() {
    let temp = tempfile::tempdir().unwrap();
    let server = spawn_models_server(json!({
        "data": [
            {"id": "qwen3-coder"},
            {"id": "deepseek-coder"}
        ]
    }));
    write_config(
        temp.path(),
        &format!(
            r#"
model = "qwen3-coder"
model_provider = "relay"

[model_providers.relay]
name = "Relay"
base_url = "{}"
experimental_bearer_token = "relay-key"
"#,
            server.base_url
        ),
    );

    let result = read_codex_model_catalog_from_home(
        temp.path(),
        &HashMap::new(),
        reqwest::Client::builder().no_proxy().build().unwrap(),
    )
    .await;

    assert_eq!(result["status"], "ok");
    assert_eq!(result["model_provider"], "relay");
    assert_eq!(result["provider_name"], "Relay");
    assert_eq!(result["default_model"], "qwen3-coder");
    assert_eq!(result["models"], json!(["qwen3-coder", "deepseek-coder"]));
    assert_eq!(
        result["sources"][0]["endpoint"],
        format!("{}/v1/models", server.base_url)
    );
    assert_eq!(
        result["responses_api"],
        json!({
            "status": "unknown",
            "endpoint": "",
            "message": ""
        })
    );
    assert_eq!(result["sources"][0]["responses_api"]["status"], "unknown");
    let requests = server.finish();
    assert_eq!(requests[0].path, "/v1/models");
    assert_eq!(requests[0].authorization, "Bearer relay-key");
}

#[tokio::test]
async fn model_catalog_appends_models_to_versioned_base_url() {
    // Volcano Engine ARK (and other providers) expose a versioned base URL such
    // as `.../api/coding/v3`. The model list must be fetched from
    // `<base>/models`, not `<base>/v1/models` (which 404s). Regression for #1349.
    let temp = tempfile::tempdir().unwrap();
    let server = spawn_models_server(json!({
        "data": [
            {"id": "doubao-seed-code-preview"}
        ]
    }));
    let versioned_base = format!("{}/api/coding/v3", server.base_url);
    write_config(
        temp.path(),
        &format!(
            r#"
model = "doubao-seed-code-preview"
model_provider = "ark"

[model_providers.ark]
name = "ARK"
base_url = "{versioned_base}"
experimental_bearer_token = "ark-key"
"#
        ),
    );

    let result = read_codex_model_catalog_from_home(
        temp.path(),
        &HashMap::new(),
        reqwest::Client::builder().no_proxy().build().unwrap(),
    )
    .await;

    assert_eq!(result["status"], "ok");
    assert_eq!(result["models"], json!(["doubao-seed-code-preview"]));
    assert_eq!(
        result["sources"][0]["endpoint"],
        format!("{versioned_base}/models")
    );
    let requests = server.finish();
    assert_eq!(requests[0].path, "/api/coding/v3/models");
    assert_eq!(requests[0].authorization, "Bearer ark-key");
}

#[tokio::test]
async fn model_catalog_reports_effective_service_tier_from_config() {
    let temp = tempfile::tempdir().unwrap();
    let server = spawn_models_server(json!({
        "data": [
            {"id": "qwen3-coder"}
        ]
    }));
    write_config(
        temp.path(),
        &format!(
            r#"
model = "qwen3-coder"
model_provider = "relay"
service_tier = "fast"

[model_providers.relay]
name = "Relay"
base_url = "{}"
experimental_bearer_token = "relay-key"
"#,
            server.base_url
        ),
    );

    let result = read_codex_model_catalog_from_home(
        temp.path(),
        &HashMap::new(),
        reqwest::Client::builder().no_proxy().build().unwrap(),
    )
    .await;

    assert_eq!(result["status"], "ok");
    assert_eq!(result["service_tier"], "fast");
    server.finish();
}

#[tokio::test]
async fn model_catalog_service_tier_is_null_when_config_does_not_set_it() {
    let temp = tempfile::tempdir().unwrap();
    let server = spawn_models_server(json!({
        "data": [
            {"id": "qwen3-coder"}
        ]
    }));
    write_config(
        temp.path(),
        &format!(
            r#"
model = "qwen3-coder"
model_provider = "relay"

[model_providers.relay]
name = "Relay"
base_url = "{}"
experimental_bearer_token = "relay-key"
"#,
            server.base_url
        ),
    );

    let result = read_codex_model_catalog_from_home(
        temp.path(),
        &HashMap::new(),
        reqwest::Client::builder().no_proxy().build().unwrap(),
    )
    .await;

    assert_eq!(result["status"], "ok");
    assert_eq!(result["service_tier"], serde_json::Value::Null);
    server.finish();
}

#[tokio::test]
async fn model_catalog_uses_active_relay_models_and_normalized_transport_provider() {
    let temp = tempfile::tempdir().unwrap();
    let codex_home = temp.path().join("codex-home");
    std::fs::create_dir_all(&codex_home).unwrap();
    let settings_path = temp.path().join("settings.json");
    let previous_codex_home = std::env::var_os("CODEX_HOME");
    let previous_settings_path =
        codex_plus_core::paths::set_settings_path_for_tests(Some(settings_path.clone()));
    unsafe {
        std::env::set_var("CODEX_HOME", &codex_home);
    }

    let (result, live_fallback_result) = async {
        write_config(
            &codex_home,
            "model = \"qwen3-coder\"\nmodel_provider = \"live_vendor\"\n",
        );
        let store = SettingsStore::new(settings_path);
        let mut settings = BackendSettings {
            active_relay_id: "relay-a".to_string(),
            relay_profiles: vec![RelayProfile {
                id: "relay-a".to_string(),
                name: "Relay A".to_string(),
                model: "qwen3-coder".to_string(),
                base_url: "https://example.test/v1".to_string(),
                protocol: RelayProtocol::Responses,
                relay_mode: RelayMode::MixedApi,
                model_list: "deepseek-coder\nqwen3-coder\nclaude-compatible\ngpt-5.6-sol"
                    .to_string(),
                config_contents: "model = \"qwen3-coder\"\nmodel_provider = \"vendor_alpha\"\n"
                    .to_string(),
                ..RelayProfile::default()
            }],
            ..BackendSettings::default()
        };
        store.save(&settings).unwrap();
        let result = read_codex_model_catalog().await;

        settings.relay_profiles[0].relay_mode = RelayMode::Official;
        settings.relay_profiles[0].official_mix_api_key = false;
        settings.relay_profiles[0].config_contents = "model = \"qwen3-coder\"\n".to_string();
        store.save(&settings).unwrap();
        let live_fallback_result = read_codex_model_catalog().await;
        (result, live_fallback_result)
    }
    .await;

    match previous_codex_home {
        Some(value) => unsafe {
            std::env::set_var("CODEX_HOME", value);
        },
        None => unsafe {
            std::env::remove_var("CODEX_HOME");
        },
    }
    codex_plus_core::paths::set_settings_path_for_tests(previous_settings_path);

    assert_eq!(result["status"], "ok");
    assert_eq!(result["model_provider"], "relay-a");
    assert_eq!(result["codex_model_provider"], "custom");
    assert_eq!(live_fallback_result["codex_model_provider"], "live_vendor");
    assert_eq!(result["provider_name"], "Relay A");
    assert_eq!(result["default_model"], "qwen3-coder");
    assert_eq!(
        result["models"],
        json!([
            "qwen3-coder",
            "deepseek-coder",
            "claude-compatible",
            "gpt-5.6-sol"
        ])
    );
    assert_eq!(
        result["modelMetadata"]["gpt-5.6-sol"]["defaultReasoningEffort"],
        "low"
    );
    assert_eq!(
        result["modelMetadata"]["gpt-5.6-sol"]["supportedReasoningEfforts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry["reasoningEffort"].as_str())
            .collect::<Vec<_>>(),
        ["low", "medium", "high", "xhigh", "max", "ultra"]
    );
    assert_eq!(result["sources"][0]["type"], "relay_profile_model_list");
}

#[tokio::test]
async fn model_catalog_displays_official_auth_before_aggregate_provider_targets() {
    let temp = tempfile::tempdir().unwrap();
    let codex_home = temp.path().join("codex-home");
    std::fs::create_dir_all(&codex_home).unwrap();
    let settings_path = temp.path().join("settings.json");
    let previous_codex_home = std::env::var_os("CODEX_HOME");
    let previous_settings_path =
        codex_plus_core::paths::set_settings_path_for_tests(Some(settings_path.clone()));
    let official_catalog_path = temp.path().join("official-model-catalog.json");
    let previous_official_catalog_path =
        codex_plus_core::paths::set_official_model_catalog_path_for_tests(Some(
            official_catalog_path.clone(),
        ));
    std::fs::write(
        &official_catalog_path,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "accounts": {
                "account-fixture": {
                    "accountId": "account-fixture",
                    "fetchedAt": 1_800_000_000i64,
                    "clientVersion": "0.153.4",
                    "etag": "fixture-etag",
                    "source": "account_snapshot",
                    "models": [
                        {
                            "slug": "gpt-6-astra",
                            "display_name": "GPT-6-Astra",
                            "description": "Fixture future official model",
                            "default_reasoning_level": "medium",
                            "supported_reasoning_levels": [
                                { "effort": "low", "description": "Fast" },
                                { "effort": "medium", "description": "Balanced" },
                                { "effort": "high", "description": "Deep" },
                                { "effort": "xhigh", "description": "Extra deep" },
                                { "effort": "max", "description": "Maximum" },
                                { "effort": "ultra", "description": "Ultra" }
                            ],
                            "additional_speed_tiers": ["fast"],
                            "service_tiers": [{ "id": "priority", "name": "Fast" }],
                            "visibility": "list",
                            "supported_in_api": true,
                            "context_window": 272000,
                            "max_context_window": 272000
                        },
                        {
                            "slug": "gpt-7-internal",
                            "display_name": "GPT-7 Internal",
                            "visibility": "hidden",
                            "supported_in_api": true
                        }
                    ]
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    unsafe {
        std::env::set_var("CODEX_HOME", &codex_home);
    }

    let result = async {
        SettingsStore::new(settings_path)
            .save(&BackendSettings {
                active_relay_id: "aggregate".to_string(),
                active_aggregate_relay_id: "aggregate".to_string(),
                official_login_mixed_mode: true,
                official_login_relay_id: "official".to_string(),
                active_official_account_id: "account-fixture".to_string(),
                relay_profiles: vec![
                    RelayProfile {
                        id: "official".to_string(),
                        name: "OpenAI".to_string(),
                        relay_mode: RelayMode::Official,
                        model_list: "gpt-6-astra".to_string(),
                        auth_contents:
                            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"official"}}"#
                                .to_string(),
                        ..RelayProfile::default()
                    },
                    RelayProfile {
                        id: "managed-cliproxy-official".to_string(),
                        name: "CLIProxyAPI 官方模型".to_string(),
                        integration_type: "cliproxy-official".to_string(),
                        model: "account-2/gpt-6-astra".to_string(),
                        model_list: "account-2/gpt-6-astra".to_string(),
                        base_url: "http://127.0.0.1:8317/v1".to_string(),
                        api_key: "cli-key".to_string(),
                        relay_mode: RelayMode::PureApi,
                        ..RelayProfile::default()
                    },
                    RelayProfile {
                        id: "managed-cliproxy".to_string(),
                        name: "CLIProxyAPI".to_string(),
                        integration_type: "cliproxy".to_string(),
                        model: "gemini-2.5-pro".to_string(),
                        model_list: "account-2/gpt-6-astra\ngemini-2.5-pro".to_string(),
                        base_url: "http://127.0.0.1:8317/v1".to_string(),
                        api_key: "cli-key".to_string(),
                        relay_mode: RelayMode::PureApi,
                        ..RelayProfile::default()
                    },
                    RelayProfile {
                        id: "provider-a".to_string(),
                        name: "供应商一".to_string(),
                        model: "gpt-6-astra".to_string(),
                        model_list: "gpt-6-astra\ngpt-5.2".to_string(),
                        base_url: "https://a.example.test/v1".to_string(),
                        api_key: "key-a".to_string(),
                        relay_mode: RelayMode::PureApi,
                        ..RelayProfile::default()
                    },
                    RelayProfile {
                        id: "provider-b".to_string(),
                        name: "供应商二".to_string(),
                        model: "vendor-gpt-6-astra".to_string(),
                        model_list: "vendor-gpt-6-astra".to_string(),
                        base_url: "https://b.example.test/v1".to_string(),
                        api_key: "key-b".to_string(),
                        relay_mode: RelayMode::PureApi,
                        ..RelayProfile::default()
                    },
                    RelayProfile {
                        id: "aggregate".to_string(),
                        name: "聚合".to_string(),
                        model: "gpt-6-astra".to_string(),
                        relay_mode: RelayMode::Aggregate,
                        ..RelayProfile::default()
                    },
                ],
                aggregate_relay_profiles: vec![codex_plus_core::settings::AggregateRelayProfile {
                    id: "aggregate".to_string(),
                    name: "聚合".to_string(),
                    session_provider: codex_plus_core::settings::RelaySessionProvider::Custom,
                    strategy: codex_plus_core::settings::AggregateRelayStrategy::Failover,
                    model_mappings_enabled: true,
                    members: vec![
                        codex_plus_core::settings::AggregateRelayMember {
                            relay_id: "provider-a".to_string(),
                            weight: 1,
                        },
                        codex_plus_core::settings::AggregateRelayMember {
                            relay_id: "provider-b".to_string(),
                            weight: 1,
                        },
                    ],
                    model_mappings: vec![codex_plus_core::settings::AggregateRelayModelMapping {
                        codex_model: "gpt-6-astra".to_string(),
                        targets: vec![
                            codex_plus_core::settings::AggregateRelayDispatchTarget {
                                relay_id: "provider-b".to_string(),
                                target_model: "vendor-gpt-6-astra".to_string(),
                            },
                            codex_plus_core::settings::AggregateRelayDispatchTarget {
                                relay_id: "provider-a".to_string(),
                                target_model: "gpt-6-astra".to_string(),
                            },
                        ],
                    }],
                    routes: Vec::new(),
                }],
                ..BackendSettings::default()
            })
            .unwrap();

        read_codex_model_catalog().await
    }
    .await;

    match previous_codex_home {
        Some(value) => unsafe {
            std::env::set_var("CODEX_HOME", value);
        },
        None => unsafe {
            std::env::remove_var("CODEX_HOME");
        },
    }
    codex_plus_core::paths::set_settings_path_for_tests(previous_settings_path);
    codex_plus_core::paths::set_official_model_catalog_path_for_tests(
        previous_official_catalog_path,
    );

    assert_eq!(result["default_model"], "gpt-6-astra");
    assert_eq!(result["model_provider"], "openai");
    assert_eq!(result["officialModels"], json!(["gpt-6-astra"]));
    assert!(
        result["trustedOfficialModels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model == "gpt-7-internal")
    );
    let models = result["models"].as_array().unwrap();
    assert!(models.iter().any(|model| model == "gpt-6-astra"));
    assert!(
        models
            .iter()
            .any(|model| model == "CLIProxyAPI:gpt-6-astra")
    );
    assert!(
        models
            .iter()
            .any(|model| model == "gpt-6-astra(供应商一|供应商二:vendor-gpt-6-astra)")
    );
    assert!(!models.iter().any(|model| model == "gpt-7-internal"));
    assert_eq!(
        models
            .iter()
            .filter(|model| *model == "gpt-6-astra")
            .count(),
        1
    );
    assert_eq!(
        result["modelMetadata"]["gpt-6-astra"]["defaultReasoningEffort"],
        "medium"
    );
    assert!(result["modelMetadata"]["gpt-6-astra"]["displaySuffix"].is_null());
    assert_eq!(
        result["modelMetadata"]["gpt-6-astra(供应商一|供应商二:vendor-gpt-6-astra)"]["displayName"],
        "gpt-6-astra(供应商一|供应商二:vendor-gpt-6-astra)"
    );
    assert!(result["modelMetadata"]["gpt-6-astra(供应商一|供应商二:vendor-gpt-6-astra)"]["displaySuffix"].is_null());
    let aggregate_route = result["routeDescriptors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|route| route["routingSlug"] == "gpt-6-astra(供应商一|供应商二:vendor-gpt-6-astra)")
        .unwrap();
    assert_eq!(
        aggregate_route["providerId"],
        "codex_plus_aggregate_aggregate"
    );
    assert_eq!(aggregate_route["capabilitySlug"], "gpt-6-astra");
    assert_eq!(
        result["modelMetadata"]["CLIProxyAPI:gpt-6-astra"]["displayName"],
        "CLIProxyAPI:gpt-6-astra"
    );
    assert!(
        result["modelMetadata"]["CLIProxyAPI:gpt-6-astra"]["priority"]
            .as_i64()
            .unwrap()
            >= 1000
    );
    assert_eq!(
        result["modelMetadata"]["CLIProxyAPI:gpt-6-astra"]["supportedReasoningEfforts"][5]["reasoningEffort"],
        "ultra"
    );
    assert_eq!(
        result["modelMetadata"]["CLIProxyAPI:gpt-6-astra"]["additionalSpeedTiers"][0],
        "fast"
    );
    assert_eq!(
        result["modelMetadata"]["CLIProxyAPI:gpt-6-astra"]["serviceTiers"][0]["id"],
        "priority"
    );
}

#[tokio::test]
async fn model_catalog_uses_single_provider_when_root_model_provider_is_absent() {
    let temp = tempfile::tempdir().unwrap();
    let server = spawn_models_server(json!({
        "models": ["moonshot-v1", "mimo-v2.5-pro"]
    }));
    write_config(
        temp.path(),
        &format!(
            r#"
[model_providers.only]
name = "Only Provider"
base_url = "{}/v1"
"#,
            server.base_url
        ),
    );

    let result = read_codex_model_catalog_from_home(
        temp.path(),
        &HashMap::new(),
        reqwest::Client::builder().no_proxy().build().unwrap(),
    )
    .await;

    assert_eq!(result["status"], "ok");
    assert_eq!(result["model_provider"], "only");
    assert_eq!(result["models"], json!(["moonshot-v1", "mimo-v2.5-pro"]));
    let requests = server.finish();
    assert_eq!(requests[0].path, "/v1/models");
    assert_eq!(result["responses_api"]["status"], "unknown");
}

#[tokio::test]
async fn model_catalog_merges_models_from_config_model_catalog_json() {
    let temp = tempfile::tempdir().unwrap();
    let server = spawn_models_server(json!({
        "data": [
            {"id": "qwen3-coder"}
        ]
    }));
    let catalog_path = temp.path().join("custom-models.json");
    std::fs::write(
        &catalog_path,
        json!({
            "models": [
                {
                    "slug": "gpt-5.6-sol",
                    "display_name": "GPT-5.6-Sol",
                    "visibility": "list",
                    "supported_in_api": true
                }
            ]
        })
        .to_string(),
    )
    .unwrap();
    write_config(
        temp.path(),
        &format!(
            r#"
 model = "gpt-5.6-sol"
model_provider = "relay"
model_catalog_json = "{}"

[model_providers.relay]
name = "Relay"
base_url = "{}"
experimental_bearer_token = "relay-key"
"#,
            catalog_path.display().to_string().replace('\\', "\\\\"),
            server.base_url
        ),
    );

    let result = read_codex_model_catalog_from_home(
        temp.path(),
        &HashMap::new(),
        reqwest::Client::builder().no_proxy().build().unwrap(),
    )
    .await;

    assert_eq!(result["status"], "ok");
    assert_eq!(result["default_model"], "gpt-5.6-sol");
    assert_eq!(result["models"], json!(["qwen3-coder", "gpt-5.6-sol"]));
    assert_eq!(
        result["modelMetadata"]["gpt-5.6-sol"]["supportedReasoningEfforts"][5]["reasoningEffort"],
        "ultra"
    );
    server.finish();
}

#[tokio::test]
async fn model_catalog_reads_single_quoted_config_model_catalog_json_path() {
    let temp = tempfile::tempdir().unwrap();
    let catalog_path = temp.path().join("literal-path-models.json");
    std::fs::write(
        &catalog_path,
        json!({
            "models": [
                {
                    "slug": "gpt-5.6",
                    "visibility": "list",
                    "supported_in_api": true
                },
                {
                    "slug": "hidden-test-model",
                    "visibility": "hidden",
                    "supported_in_api": true
                },
                {
                    "slug": "chatgpt-only-test-model",
                    "visibility": "list",
                    "supported_in_api": false
                }
            ]
        })
        .to_string(),
    )
    .unwrap();
    write_config(
        temp.path(),
        &format!(
            r#"
model = "gpt-5.6"
model_catalog_json = '{}'
"#,
            catalog_path.display()
        ),
    );

    let result = read_codex_model_catalog_from_home(
        temp.path(),
        &HashMap::new(),
        reqwest::Client::builder().no_proxy().build().unwrap(),
    )
    .await;

    assert_eq!(result["status"], "ok");
    assert_eq!(result["default_model"], "gpt-5.6");
    assert_eq!(result["models"], json!(["gpt-5.6"]));
    assert_eq!(result["sources"][0]["status"], "ok");
    assert_eq!(result["sources"][0]["models"], 1);
}

#[tokio::test]
async fn model_catalog_leaves_responses_api_unknown_without_probe() {
    let temp = tempfile::tempdir().unwrap();
    let server = spawn_models_server(json!({
        "data": [
            {"id": "legacy-model"}
        ]
    }));
    write_config(
        temp.path(),
        &format!(
            r#"
model = "legacy-model"

[model_providers.legacy]
name = "Legacy"
base_url = "{}"
"#,
            server.base_url
        ),
    );

    let result = read_codex_model_catalog_from_home(
        temp.path(),
        &HashMap::new(),
        reqwest::Client::builder().no_proxy().build().unwrap(),
    )
    .await;

    assert_eq!(result["status"], "ok");
    assert_eq!(result["responses_api"]["status"], "unknown");
    assert_eq!(result["responses_api"]["endpoint"], "");
    assert_eq!(result["sources"][0]["responses_api"]["status"], "unknown");
    let requests = server.finish();
    assert_eq!(requests[0].path, "/v1/models");
}

fn write_config(home: &Path, contents: &str) {
    std::fs::write(home.join("config.toml"), contents.trim_start()).unwrap();
}

struct ModelsServer {
    base_url: String,
    handle: thread::JoinHandle<Vec<ModelsRequest>>,
}

impl ModelsServer {
    fn finish(self) -> Vec<ModelsRequest> {
        self.handle.join().unwrap()
    }
}

struct ModelsRequest {
    path: String,
    authorization: String,
}

fn spawn_models_server(payload: serde_json::Value) -> ModelsServer {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    listener
        .set_nonblocking(true)
        .expect("listener should switch to nonblocking mode");
    let models_body = payload.to_string();
    let handle = thread::spawn(move || {
        let started = std::time::Instant::now();
        let mut requests = Vec::new();
        while requests.is_empty() && started.elapsed() < std::time::Duration::from_secs(2) {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            };
            let mut buffer = [0u8; 4096];
            let mut read = 0;
            let read_started = std::time::Instant::now();
            while read == 0 && read_started.elapsed() < std::time::Duration::from_secs(2) {
                match stream.read(&mut buffer) {
                    Ok(0) => std::thread::sleep(std::time::Duration::from_millis(10)),
                    Ok(bytes) => read = bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("failed to read test request: {error}"),
                }
            }
            if read == 0 {
                continue;
            }
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            let request_path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or_default()
                .to_string();
            let authorization = request
                .lines()
                .find_map(|line| line.strip_prefix("authorization: "))
                .unwrap_or_default()
                .to_string();
            let (status, body) = (200, models_body.as_str());
            let response = format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
            requests.push(ModelsRequest {
                path: request_path,
                authorization,
            });
        }
        requests
    });
    ModelsServer { base_url, handle }
}
