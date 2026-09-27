use codex_plus_core::official_experience::{
    aggregate_provider_id, build_model_catalog, descriptor_for_provider_and_model,
    local_route_base_url, model_route_descriptors, relay_provider_id, route_operation_path,
    route_provider_id_from_path,
};
use codex_plus_core::protocol_proxy::open_responses_proxy_request_with_settings_for_path;
use codex_plus_core::relay_config::ensure_official_experience_config_in_home;
use codex_plus_core::settings::{
    AggregateRelayMember, AggregateRelayProfile, BackendSettings, ModelCapabilityBinding,
    OfficialExperienceSourceKind, RelayMode, RelayProfile, RelayProtocol,
};

fn sample_settings() -> (BackendSettings, String) {
    let mut settings = BackendSettings::default();
    settings.official_experience.enabled = true;
    let official_slug =
        codex_plus_core::official_model_catalog::visible_official_model_slugs_for_settings(
            &settings,
        )
        .into_iter()
        .next()
        .expect("bundled official catalog must have a visible model");
    settings.relay_profiles = vec![RelayProfile {
        id: "vendor-a".to_string(),
        name: "供应商 A".to_string(),
        relay_mode: RelayMode::PureApi,
        base_url: "https://vendor.example/v1".to_string(),
        upstream_base_url: "https://vendor.example/v1".to_string(),
        api_key: "test-key".to_string(),
        model_list: format!("{official_slug}\nmodel-x"),
        ..RelayProfile::default()
    }];
    settings.active_relay_id = "vendor-a".to_string();
    settings.official_experience.capability_bindings = vec![ModelCapabilityBinding {
        routing_slug: "vendor-a:model-x".to_string(),
        capability_slug: official_slug.clone(),
        source_kind: OfficialExperienceSourceKind::Relay,
        source_id: "vendor-a".to_string(),
        upstream_model: "model-x".to_string(),
    }];
    (settings, official_slug)
}

#[test]
fn one_capability_template_keeps_each_source_model_distinguishable() {
    let (mut settings, official_slug) = sample_settings();
    settings.relay_profiles[0].model_list.push_str("\nmodel-y");
    settings
        .official_experience
        .capability_bindings
        .push(ModelCapabilityBinding {
            routing_slug: "vendor-a:model-y".to_string(),
            capability_slug: official_slug.clone(),
            source_kind: OfficialExperienceSourceKind::Relay,
            source_id: "vendor-a".to_string(),
            upstream_model: "model-y".to_string(),
        });
    settings.official_experience.cli_general_enabled = true;
    settings.relay_profiles.push(RelayProfile {
        id: "managed-cliproxy".to_string(),
        name: "CLIProxyAPI".to_string(),
        integration_type: "cliproxy".to_string(),
        relay_mode: RelayMode::PureApi,
        model_list: "gemini-2.5-pro".to_string(),
        ..RelayProfile::default()
    });
    settings
        .official_experience
        .capability_bindings
        .push(ModelCapabilityBinding {
            routing_slug: "CLIProxyAPI:gemini-2.5-pro".to_string(),
            capability_slug: official_slug.clone(),
            source_kind: OfficialExperienceSourceKind::CliGeneral,
            source_id: "managed-cliproxy".to_string(),
            upstream_model: "gemini-2.5-pro".to_string(),
        });

    let descriptors = model_route_descriptors(&settings);
    for (routing_slug, display_name, upstream_model, source_kind) in [
        (
            "vendor-a:model-x",
            "供应商 A:model-x",
            "model-x",
            OfficialExperienceSourceKind::Relay,
        ),
        (
            "vendor-a:model-y",
            "供应商 A:model-y",
            "model-y",
            OfficialExperienceSourceKind::Relay,
        ),
        (
            "CLIProxyAPI:gemini-2.5-pro",
            "CLIProxyAPI:gemini-2.5-pro",
            "gemini-2.5-pro",
            OfficialExperienceSourceKind::CliGeneral,
        ),
    ] {
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.routing_slug == routing_slug)
            .unwrap();
        assert_eq!(descriptor.display_name, display_name);
        assert_eq!(descriptor.upstream_model, upstream_model);
        assert_eq!(descriptor.capability_slug, official_slug);
        assert_eq!(descriptor.source_kind, source_kind);
        assert_eq!(descriptor.failover_policy, "strict");
    }
    let catalog = build_model_catalog(&settings);
    let models = catalog["models"].as_array().unwrap();
    for (slug, display_name) in [
        ("vendor-a:model-x", "供应商 A:model-x"),
        ("vendor-a:model-y", "供应商 A:model-y"),
        ("CLIProxyAPI:gemini-2.5-pro", "CLIProxyAPI:gemini-2.5-pro"),
    ] {
        assert_eq!(
            models.iter().find(|model| model["slug"] == slug).unwrap()["display_name"],
            display_name
        );
    }
}

#[test]
fn cliproxy_api_keeps_its_route_name_and_follows_official_gpt_models() {
    let (mut settings, official_slug) = sample_settings();
    settings.official_experience.cli_official_enabled = true;
    settings.relay_profiles.push(RelayProfile {
        id: "managed-cliproxy-official".to_string(),
        name: "CLIProxyAPI".to_string(),
        integration_type:
            codex_plus_core::aggregate_model_alias::CLIPROXY_OFFICIAL_INTEGRATION_TYPE.to_string(),
        relay_mode: RelayMode::PureApi,
        model_list: official_slug.clone(),
        ..RelayProfile::default()
    });

    let alias = format!("CLIProxyAPI:{official_slug}");
    let routes = model_route_descriptors(&settings);
    let official_index = routes
        .iter()
        .position(|route| route.routing_slug == official_slug)
        .unwrap();
    let cli_index = routes
        .iter()
        .position(|route| route.routing_slug == alias)
        .unwrap();
    let relay_index = routes
        .iter()
        .position(|route| route.routing_slug == "vendor-a:model-x")
        .unwrap();
    assert!(official_index < cli_index && cli_index < relay_index);
    assert_eq!(routes[cli_index].display_name, alias);
    assert_eq!(routes[cli_index].provider_id, "codex_plus_cli_official");
    assert_eq!(routes[cli_index].upstream_model, official_slug);

    let catalog = build_model_catalog(&settings);
    let models = catalog["models"].as_array().unwrap();
    let priority = |slug: &str| {
        models
            .iter()
            .find(|model| model["slug"] == slug)
            .and_then(|model| model["priority"].as_i64())
            .unwrap()
    };
    assert!(priority(&official_slug) < priority(&alias));
    assert!(priority(&alias) < priority("vendor-a:model-x"));
    assert_eq!(
        models.iter().find(|model| model["slug"] == alias).unwrap()["display_name"],
        alias
    );
}

#[test]
fn selected_aggregate_exposes_only_members_and_rejects_stale_bindings() {
    use codex_plus_core::settings::{
        AggregateRelayDispatchTarget, AggregateRelayMember, AggregateRelayModelMapping,
        AggregateRelayProfile,
    };

    let (mut settings, official_slug) = sample_settings();
    settings.relay_profiles.push(RelayProfile {
        id: "vendor-b".to_string(),
        name: "供应商 B".to_string(),
        relay_mode: RelayMode::PureApi,
        model_list: "unused-model".to_string(),
        ..RelayProfile::default()
    });
    settings.relay_profiles.push(RelayProfile {
        id: "selected-aggregate".to_string(),
        name: "选中聚合".to_string(),
        relay_mode: RelayMode::Aggregate,
        ..RelayProfile::default()
    });
    settings
        .aggregate_relay_profiles
        .push(AggregateRelayProfile {
            id: "selected-aggregate".to_string(),
            name: "选中聚合".to_string(),
            session_provider: Default::default(),
            strategy: Default::default(),
            model_mappings_enabled: true,
            members: vec![AggregateRelayMember {
                relay_id: "vendor-a".to_string(),
                weight: 1,
            }],
            model_mappings: Vec::new(),
            routes: Vec::new(),
        });
    settings.active_relay_id = "selected-aggregate".to_string();
    settings.active_aggregate_relay_id = "selected-aggregate".to_string();
    settings.official_experience.capability_bindings = vec![
        ModelCapabilityBinding {
            routing_slug: "供应商 A:model-x".to_string(),
            capability_slug: official_slug.clone(),
            source_kind: OfficialExperienceSourceKind::Aggregate,
            source_id: "selected-aggregate".to_string(),
            upstream_model: "供应商 A:model-x".to_string(),
        },
        ModelCapabilityBinding {
            routing_slug: "vendor-b:unused-model".to_string(),
            capability_slug: official_slug.clone(),
            source_kind: OfficialExperienceSourceKind::Relay,
            source_id: "vendor-b".to_string(),
            upstream_model: "unused-model".to_string(),
        },
    ];
    let descriptors = model_route_descriptors(&settings);
    assert!(
        descriptors
            .iter()
            .any(|route| route.routing_slug == "供应商 A:model-x"
                && route.display_name == "供应商 A:model-x"
                && route.source_kind == OfficialExperienceSourceKind::Aggregate)
    );
    assert!(
        !descriptors
            .iter()
            .any(|route| route.routing_slug.starts_with("vendor-a:")
                || route.routing_slug.starts_with("vendor-b:"))
    );
    settings.aggregate_relay_profiles[0]
        .model_mappings
        .push(AggregateRelayModelMapping {
            codex_model: official_slug.clone(),
            targets: vec![AggregateRelayDispatchTarget {
                relay_id: "vendor-a".to_string(),
                target_model: "model-x".to_string(),
            }],
        });
    let replacement_alias = format!("{official_slug}(供应商 A:model-x)");
    let mapped = model_route_descriptors(&settings)
        .into_iter()
        .find(|route| route.routing_slug == replacement_alias)
        .expect("aggregate replacement route");
    assert_eq!(mapped.display_name, replacement_alias);
    assert_eq!(mapped.upstream_model, replacement_alias);
    assert_eq!(mapped.capability_slug, official_slug);
    assert_eq!(mapped.source_kind, OfficialExperienceSourceKind::Aggregate);
    settings.aggregate_relay_profiles[0].members.clear();
    assert!(
        !model_route_descriptors(&settings)
            .iter()
            .any(|route| route.routing_slug == "供应商 A:model-x")
    );
    settings.active_relay_id = "vendor-a".to_string();
    settings.active_aggregate_relay_id.clear();
    let direct_routes = model_route_descriptors(&settings);
    assert!(
        direct_routes
            .iter()
            .any(|route| route.routing_slug.starts_with("vendor-a:"))
    );
    assert!(
        !direct_routes
            .iter()
            .any(|route| route.source_kind == OfficialExperienceSourceKind::Aggregate)
    );
}

#[test]
fn source_provider_ids_and_paths_are_stable_and_unambiguous() {
    assert_ne!(relay_provider_id("vendor-a"), relay_provider_id("vendor_a"));
    assert_ne!(relay_provider_id("Vendor"), relay_provider_id("vendor"));
    let provider = relay_provider_id("vendor-a");
    let base = local_route_base_url(57321, &provider);
    let path = format!("/v1/routes/{provider}/responses/compact");
    assert_eq!(base, format!("http://127.0.0.1:57321/v1/routes/{provider}"));
    assert_eq!(
        route_provider_id_from_path(&path).as_deref(),
        Some(provider.as_str())
    );
    assert_eq!(route_operation_path(&path), "/v1/responses/compact");
    assert_eq!(
        route_operation_path(&format!("/v1/routes/{provider}/chat/completions")),
        format!("/v1/routes/{provider}/chat/completions")
    );
}

#[test]
fn exact_official_template_is_copied_into_hidden_source_entries() {
    let (settings, official_slug) = sample_settings();
    let descriptors = model_route_descriptors(&settings);
    let provider = relay_provider_id("vendor-a");
    assert!(descriptors.iter().any(|descriptor| {
        descriptor.routing_slug == official_slug && descriptor.provider_id == "openai"
    }));
    let mapped = descriptor_for_provider_and_model(&settings, &provider, "vendor-a:model-x")
        .expect("explicit model binding");
    assert_eq!(mapped.capability_slug, official_slug);
    assert_eq!(mapped.upstream_model, "model-x");
    assert_eq!(mapped.failover_policy, "strict");
    assert!(descriptor_for_provider_and_model(&settings, &provider, &official_slug).is_none());

    let catalog = build_model_catalog(&settings);
    let models = catalog["models"].as_array().unwrap();
    let official = models
        .iter()
        .find(|model| model["slug"] == official_slug)
        .unwrap();
    let extended = models
        .iter()
        .find(|model| model["slug"] == "vendor-a:model-x")
        .unwrap();
    for key in [
        "supported_reasoning_levels",
        "default_reasoning_level",
        "additional_speed_tiers",
        "context_window",
        "supports_parallel_tool_calls",
        "base_instructions",
        "experimental_supported_tools",
    ] {
        assert_eq!(extended[key], official[key], "capability field {key}");
    }
    assert_eq!(extended["visibility"], "hide");
    assert_eq!(extended["prefer_websockets"], false);
}

#[test]
fn migration_keeps_root_official_and_preserves_user_context() {
    let (settings, official_slug) = sample_settings();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let original = "model_provider = \"custom\"\nmodel = \"third-party-model\"\nopenai_base_url = \"http://127.0.0.1:57321/v1\"\n\n[model_providers.custom]\nbase_url = \"http://127.0.0.1:57321/v1\"\nwire_api = \"responses\"\n\n[model_providers.openai]\nbase_url = \"http://127.0.0.1:57321/v1\"\n\n[mcp_servers.demo]\ncommand = \"demo\"\n";
    std::fs::write(home.join("config.toml"), original).unwrap();

    assert!(ensure_official_experience_config_in_home(home, &settings).unwrap());
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    let doc = config.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(doc["model_provider"].as_str(), Some("openai"));
    assert_eq!(doc["model"].as_str(), Some(official_slug.as_str()));
    assert!(doc.get("openai_base_url").is_none());
    assert!(doc["model_providers"]["openai"].get("base_url").is_none());
    assert_eq!(doc["mcp_servers"]["demo"]["command"].as_str(), Some("demo"));
    assert_eq!(
        doc["model_providers"][relay_provider_id("vendor-a").as_str()]["base_url"].as_str(),
        Some(local_route_base_url(57321, &relay_provider_id("vendor-a")).as_str()),
    );
    assert_eq!(
        doc["model_providers"]["custom"]["base_url"].as_str(),
        Some("http://127.0.0.1:57321/v1"),
    );
    let catalog =
        std::fs::read_to_string(home.join("codex-plus-official-experience-model-catalog.json"))
            .unwrap();
    assert!(catalog.contains("vendor-a:model-x"));
    let backups = std::fs::read_dir(home)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("config.toml.bak.")
        })
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        std::fs::read_to_string(backups[0].path()).unwrap(),
        original
    );
    assert!(!ensure_official_experience_config_in_home(home, &settings).unwrap());
}

#[test]
fn extension_route_providers_must_not_use_chatgpt_account_auth() {
    // 回归：非官方通道（聚合 / CLIProxyAPI / `供应商:模型`）的 route provider 一旦带
    // `requires_openai_auth = true`，Codex 会把会话当成 ChatGPT 账号会话，
    // 只接受官方模型名，非官方模型会被拒绝并报
    // “The 'X' model is not supported when using Codex with a ChatGPT account.”。
    let (settings, _official_slug) = sample_settings();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let provider = relay_provider_id("vendor-a");
    let original = format!(
        "model_provider = \"{provider}\"\nmodel = \"vendor-a:model-x\"\n\n[model_providers.openai]\n"
    );
    std::fs::write(home.join("config.toml"), &original).unwrap();

    assert!(ensure_official_experience_config_in_home(home, &settings).unwrap());
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    let doc = config.parse::<toml_edit::DocumentMut>().unwrap();

    // 官方登录是根 provider；扩展模型只能在会话请求中显式选择专属 provider。
    assert_eq!(doc["model_provider"].as_str(), Some("openai"));
    // 扩展通道不得使用 ChatGPT 账号鉴权，并给出占位 bearer。
    assert_eq!(
        doc["model_providers"][provider.as_str()]["requires_openai_auth"].as_bool(),
        Some(false)
    );
    assert_eq!(
        doc["model_providers"][provider.as_str()]["experimental_bearer_token"].as_str(),
        Some(codex_plus_protocol_proxy_bearer_token())
    );
    // 根默认模型必须与主官方 provider 匹配，不能使普通新任务继承第三方路由。
    assert_ne!(doc["model"].as_str(), Some("vendor-a:model-x"));
    assert!(
        codex_plus_core::official_model_catalog::visible_official_model_slugs_for_settings(
            &settings
        )
        .iter()
        .any(|slug| doc["model"].as_str() == Some(slug.as_str()))
    );
}

#[test]
fn official_experience_preserves_unmanaged_custom_provider() {
    let (settings, _) = sample_settings();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let original_custom_url = "https://user-owned.example/v1";
    std::fs::write(
        home.join("config.toml"),
        format!(
            "model_provider = \"custom\"\n\n[model_providers.custom]\nbase_url = \"{original_custom_url}\"\nwire_api = \"responses\"\n"
        ),
    )
    .unwrap();

    ensure_official_experience_config_in_home(home, &settings).unwrap();
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    let doc = config.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(doc["model_provider"].as_str(), Some("openai"));
    assert_eq!(
        doc["model_providers"]["custom"]["base_url"].as_str(),
        Some(original_custom_url)
    );
    assert!(
        doc["model_providers"]["custom"]
            .get("experimental_bearer_token")
            .is_none()
    );
}

fn codex_plus_protocol_proxy_bearer_token() -> &'static str {
    codex_plus_core::protocol_proxy::NO_AUTH_PROXY_BEARER_TOKEN
}

#[tokio::test]
async fn source_route_uses_its_own_target_and_rejects_wrong_provider() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (mut settings, _) = sample_settings();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    settings.relay_profiles[0].base_url = format!("http://{addr}/v1");
    settings.relay_profiles[0].upstream_base_url = format!("http://{addr}/v1");
    let provider = relay_provider_id("vendor-a");
    let wrong_provider = relay_provider_id("vendor-b");
    let body = r#"{"model":"vendor-a:model-x","input":"hello","stream":false}"#;

    let wrong = open_responses_proxy_request_with_settings_for_path(
        body,
        settings.clone(),
        &format!("/v1/routes/{wrong_provider}/responses"),
    )
    .await
    .err()
    .expect("a model cannot use another source provider");
    assert!(wrong.to_string().contains("不属于来源 provider"));

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = stream.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
            let text = String::from_utf8_lossy(&request);
            if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                if body.len() >= length {
                    break;
                }
            }
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 35\r\n\r\n{\"id\":\"resp_1\",\"object\":\"response\"}")
            .await
            .unwrap();
        String::from_utf8(request).unwrap()
    });
    let result = open_responses_proxy_request_with_settings_for_path(
        body,
        settings,
        &format!("/v1/routes/{provider}/responses"),
    )
    .await
    .unwrap();
    assert_eq!(result.status_code, 200);
    let request = server.await.unwrap();
    assert!(request.starts_with("POST /v1/responses HTTP/1.1"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-key")
    );
    assert!(request.contains("\"model\":\"model-x\""));
    assert!(!request.contains("vendor-a:model-x"));
}

#[tokio::test]
async fn aggregate_deepseek_alias_uses_selected_member_and_never_fails_over() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (mut settings, official_slug) = sample_settings();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    settings.relay_profiles = vec![
        RelayProfile {
            id: "ecnu-id".to_string(),
            name: "Chat ECNU".to_string(),
            relay_mode: RelayMode::PureApi,
            base_url: "http://127.0.0.1:1/v1".to_string(),
            api_key: "ecnu-test-key".to_string(),
            model_list: "ecnu-plus".to_string(),
            ..RelayProfile::default()
        },
        RelayProfile {
            id: "deepseek-id".to_string(),
            name: "deepseek".to_string(),
            relay_mode: RelayMode::PureApi,
            protocol: RelayProtocol::ChatCompletions,
            base_url: format!("http://{addr}/v1"),
            api_key: "deepseek-test-key".to_string(),
            model_list: "deepseek-v4-flash\ndeepseek-v4-pro".to_string(),
            config_contents: "model = \"deepseek-v4-flash\"\n".to_string(),
            ..RelayProfile::default()
        },
        RelayProfile {
            id: "outside-id".to_string(),
            name: "outside".to_string(),
            relay_mode: RelayMode::PureApi,
            model_list: "outside-model".to_string(),
            ..RelayProfile::default()
        },
    ];
    let aggregate_id = "aggregate-test";
    settings.aggregate_relay_profiles = vec![AggregateRelayProfile {
        id: aggregate_id.to_string(),
        name: "selected aggregate".to_string(),
        session_provider: Default::default(),
        strategy: Default::default(),
        model_mappings_enabled: true,
        members: vec![
            AggregateRelayMember {
                relay_id: "ecnu-id".to_string(),
                weight: 1,
            },
            AggregateRelayMember {
                relay_id: "deepseek-id".to_string(),
                weight: 1,
            },
        ],
        model_mappings: Vec::new(),
        routes: Vec::new(),
    }];
    settings.active_relay_id = aggregate_id.to_string();
    settings.active_aggregate_relay_id = aggregate_id.to_string();
    settings.relay_profiles.push(RelayProfile {
        id: aggregate_id.to_string(),
        name: "selected aggregate".to_string(),
        relay_mode: RelayMode::Aggregate,
        ..RelayProfile::default()
    });
    settings.official_experience.capability_bindings = ["deepseek-v4-flash", "deepseek-v4-pro"]
        .into_iter()
        .map(|model| ModelCapabilityBinding {
            routing_slug: format!("deepseek:{model}"),
            capability_slug: official_slug.clone(),
            source_kind: OfficialExperienceSourceKind::Aggregate,
            source_id: aggregate_id.to_string(),
            upstream_model: format!("deepseek:{model}"),
        })
        .collect();

    let provider = aggregate_provider_id(aggregate_id);
    assert!(provider.starts_with("codex_plus_aggregate_"));
    let descriptors = model_route_descriptors(&settings);
    assert!(descriptors.iter().any(|route| {
        route.routing_slug == "deepseek:deepseek-v4-flash"
            && route.provider_id == provider
            && route.source_kind == OfficialExperienceSourceKind::Aggregate
    }));
    assert!(
        !descriptors
            .iter()
            .any(|route| route.routing_slug == "outside:outside-model")
    );

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = stream.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
            let text = String::from_utf8_lossy(&request);
            if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                if body.len() >= length {
                    break;
                }
            }
        }
        let response = b"{\"error\":{\"message\":\"source unavailable\"}}";
        stream
            .write_all(
                format!(
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    response.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(response).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    let result = open_responses_proxy_request_with_settings_for_path(
        r#"{"model":"deepseek:deepseek-v4-pro","input":"hello","stream":false}"#,
        settings,
        &format!("/v1/routes/{provider}/responses"),
    )
    .await
    .unwrap();
    assert_eq!(result.status_code, 503);
    let request = server.await.unwrap();
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer deepseek-test-key")
    );
    assert!(request.contains("\"model\":\"deepseek-v4-pro\""));
    assert!(!request.contains("deepseek-v4-flash"));
    assert!(!request.contains("ecnu-test-key"));
}
