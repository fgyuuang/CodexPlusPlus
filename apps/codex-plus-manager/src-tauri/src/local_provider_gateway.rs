//! 本地服务只负责连接资料；供应商、模型与选路复用原生网关。
use anyhow::{Context, ensure};
use codex_plus_core::gateway::{
    apply,
    store::{GatewayProvider, GatewaySettings, GatewayStore},
};
use codex_plus_core::settings::RelayProtocol;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::Mutex, time::Duration};

static APPLY_LOCK: Mutex<()> = Mutex::new(());
const MAX_BODY: usize = 1024 * 1024;

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    base_url: String,
    api_key: String,
    model: String,
    protocol: Option<RelayProtocol>,
}

fn provider<'a>(gateway: &'a GatewaySettings, service: &str) -> Option<&'a GatewayProvider> {
    gateway
        .providers
        .iter()
        .find(|item| item.source == service)
        .or_else(|| {
            gateway
                .providers
                .iter()
                .find(|item| item.source == format!("codex:managed-{service}"))
        })
}

fn summary(gateway: &GatewaySettings, service: &str) -> Value {
    match provider(gateway, service) {
        Some(item) => json!({"status":"ok", "configured":true, "providerId":item.relay.id,
            "providerName":item.relay.name, "enabled":item.enabled, "source":item.source,
            "baseUrl":item.relay.upstream_base_url, "protocol":item.relay.protocol,
            "model":item.relay.model, "models":codex_plus_core::gateway::route::relay_models(&item.relay),
            "hasApiKey":!item.relay.api_key.is_empty()}),
        None => json!({"status":"ok", "configured":false, "enabled":false}),
    }
}

pub(crate) fn workbuddy_url(raw: &str) -> anyhow::Result<String> {
    let raw = if raw.trim().is_empty() {
        "http://127.0.0.1:7864/v1"
    } else {
        raw.trim()
    };
    let mut url = reqwest::Url::parse(raw).context("WorkBuddy 地址格式无效")?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && matches!(url.path().trim_end_matches('/'), "" | "/v1"),
        "WorkBuddy 地址仅支持本机 http/https 根路径或 /v1，不能带凭据和查询参数"
    );
    url.set_path("/v1");
    Ok(url.into())
}

fn connection(
    service: &str,
    input: &Request,
    gateway: &GatewaySettings,
) -> anyhow::Result<(String, String)> {
    match service {
        "cliproxy" => crate::cliproxy::gateway_connection(),
        "newapi" => crate::newapi::gateway_connection(),
        "workbuddy" => {
            let existing = provider(gateway, service);
            let base = if input.base_url.trim().is_empty() {
                existing
                    .map(|item| item.relay.upstream_base_url.as_str())
                    .unwrap_or_default()
            } else {
                &input.base_url
            };
            let key = if input.api_key.trim().is_empty() {
                existing
                    .map(|item| item.relay.api_key.clone())
                    .unwrap_or_default()
            } else {
                input.api_key.trim().to_string()
            };
            // 换地址时必须重新给凭据，不能把原服务的 Key 静默发送给另一端。
            let base = workbuddy_url(base)?;
            if let Some(existing) = existing {
                ensure!(
                    !input.api_key.trim().is_empty()
                        || workbuddy_url(&existing.relay.upstream_base_url)? == base,
                    "WorkBuddy 地址已变化，请重新填写该服务的 API Key"
                );
            }
            Ok((base, key))
        }
        _ => anyhow::bail!("未知本地服务"),
    }
}

fn client(timeout: u64) -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(timeout))
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

async fn models(base: &str, key: &str) -> anyhow::Result<Vec<String>> {
    let value = get_json(base, key, "models").await?;
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .context("模型目录缺少 data 列表")?;
    let mut result = Vec::new();
    for item in data {
        if let Some(id) = item.get("id").and_then(Value::as_str).map(str::trim)
            && !id.is_empty()
            && id.len() <= 200
            && !id.chars().any(char::is_control)
            && !result.iter().any(|known| known == id)
        {
            result.push(id.to_string());
        }
    }
    ensure!(!result.is_empty(), "服务未返回可用模型，未修改供应商");
    Ok(result)
}

async fn get_json(base: &str, key: &str, path: &str) -> anyhow::Result<Value> {
    ensure!(
        !key.trim().is_empty(),
        "请先保存服务的 API Key（模型调用密钥）"
    );
    let mut response = client(10)?
        .get(format!("{}/{path}", base.trim_end_matches('/')))
        .bearer_auth(key)
        .send()
        .await
        .context("无法读取本地服务，请检查地址和运行状态")?;
    ensure!(
        response.status().is_success(),
        "本地服务查询失败：HTTP {}",
        response.status().as_u16()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= MAX_BODY,
            "本地服务响应超过 1 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).context("本地服务没有返回有效 JSON")
}

fn lane_model(model: &str, realm: &str) -> bool {
    match (realm, model.split_once(':')) {
        ("cn", None) => true,
        (realm, Some((prefix, name))) => prefix == realm && !name.is_empty(),
        _ => false,
    }
}

fn validate_lane_usage(usage: &Value, realm: &str) -> anyhow::Result<()> {
    ensure!(
        usage.get("object").and_then(Value::as_str) == Some("key.usage"),
        "本地服务不是支持用量查询的 WorkBuddy Manager"
    );
    ensure!(
        usage.pointer("/key/realm").and_then(Value::as_str) == Some(realm),
        "请选择此版本专用的 WorkBuddy 调用 Key，不能用另一版本或不限版本的 Key"
    );
    Ok(())
}

pub(crate) async fn validate_workbuddy_key(
    base: &str,
    key: &str,
    realm: &str,
) -> anyhow::Result<()> {
    let usage = get_json(&workbuddy_url(base)?, key.trim(), "usage").await?;
    validate_lane_usage(&usage, realm)
}

/// Verify the actual Key's realm before creating a native provider; a name alone
/// cannot isolate routing, balance or billing.
pub async fn create_workbuddy_lane(
    store: &GatewayStore,
    realm: &str,
    base_url: &str,
    key: &str,
    skip_agents: Option<&[String]>,
) -> anyhow::Result<(GatewaySettings, String)> {
    let (source, name) = match realm {
        "cn" => ("workbuddy:cn", "WorkBuddy 国内版"),
        "global" => ("workbuddy:global", "WorkBuddy 国际版"),
        _ => anyhow::bail!("未知的 WorkBuddy 版本"),
    };
    let base = workbuddy_url(base_url)?;
    let key = key.trim();
    validate_workbuddy_key(&base, key, realm).await?;
    let mut ids = models(&base, key).await?;
    ids.retain(|id| lane_model(id, realm));
    let model = ids.first().context("此版本没有可用模型")?.clone();
    let input = apply::ProviderInput {
        name: name.into(),
        base_url: base,
        protocol: RelayProtocol::Responses,
        keys: vec![key.into()],
        model,
        model_list: ids.join("\n"),
        ..Default::default()
    };
    let _guard = APPLY_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    ensure!(
        provider(&store.load()?, source).is_none(),
        "这个版本已经接入，请在供应商池中编辑已有条目"
    );
    apply::create_provider(store, &input, source, skip_agents)
}

fn upsert(
    store: &GatewayStore,
    service: &str,
    base: &str,
    key: &str,
    models: &[String],
    request: &Request,
) -> anyhow::Result<Value> {
    let _guard = APPLY_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let gateway = store.load()?;
    let existing = provider(&gateway, service);
    let name = match service {
        "cliproxy" => "CLIProxyAPI",
        "newapi" => "NewAPI",
        _ => "WorkBuddy",
    };
    let selected = if !request.model.trim().is_empty() {
        request.model.trim()
    } else {
        existing
            .map(|item| item.relay.model.as_str())
            .filter(|model| models.iter().any(|id| id == model))
            .unwrap_or(&models[0])
    };
    ensure!(
        models.iter().any(|model| model == selected),
        "所选模型已不在服务目录，请重新读取模型"
    );
    let input = apply::ProviderInput {
        name: existing
            .map(|item| item.relay.name.clone())
            .unwrap_or_else(|| name.into()),
        base_url: base.into(),
        protocol: request
            .protocol
            .or_else(|| existing.map(|item| item.relay.protocol))
            .unwrap_or(RelayProtocol::Responses),
        keys: if existing.is_some() {
            vec![]
        } else {
            vec![key.into()]
        },
        price: existing.and_then(|item| item.price.clone()),
        model: selected.into(),
        model_list: models.join("\n"),
        ..Default::default()
    };
    let saved = if let Some(existing) = existing {
        let saved = apply::update_provider(store, &existing.relay.id, &input)?;
        if existing.relay.api_key != key {
            let key_id = existing
                .relay
                .selected_api_key()
                .map(|entry| entry.id.as_str())
                .unwrap_or("default");
            apply::replace_provider_key(store, &existing.relay.id, key_id, key)?
        } else {
            saved
        }
    } else {
        apply::create_provider(store, &input, service, None)?.0
    };
    let mut result = summary(&saved, service);
    result["message"] = json!(format!(
        "已{}供应商，{} 个模型",
        if existing.is_some() {
            "更新"
        } else {
            "接入"
        },
        models.len()
    ));
    Ok(result)
}

/// 流式测试必须收到协议终止事件；HTTP 200 与中途断流都不能冒充成功。
async fn test_stream(
    base: &str,
    key: &str,
    model: &str,
    protocol: RelayProtocol,
) -> anyhow::Result<Value> {
    ensure!(!model.trim().is_empty(), "请先选择测试模型");
    let (path, body) = match protocol {
        RelayProtocol::Responses => (
            "responses",
            json!({"model":model,"input":"Reply OK.","stream":true,"max_output_tokens":32}),
        ),
        RelayProtocol::ChatCompletions => (
            "chat/completions",
            json!({"model":model,"messages":[{"role":"user","content":"Reply OK."}],"stream":true,"max_tokens":32}),
        ),
        _ => anyhow::bail!("此服务测试仅支持 Responses 或 Chat Completions"),
    };
    let mut response = client(45)?
        .post(format!("{}/{path}", base.trim_end_matches('/')))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .context("测试连接失败，请检查服务状态")?;
    ensure!(
        response.status().is_success(),
        "测试请求失败：HTTP {}",
        response.status().as_u16()
    );
    ensure!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream")),
        "服务未返回 SSE 流"
    );
    let mut pending = Vec::new();
    let mut received = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .context("测试流中断，请检查服务日志")?
    {
        received += chunk.len();
        ensure!(received <= MAX_BODY, "测试响应超过 1 MiB");
        pending.extend_from_slice(&chunk);
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let line = String::from_utf8_lossy(&pending[..end]).trim().to_string();
            pending.drain(..=end);
            if let Some(data) = line.strip_prefix("data:").map(str::trim) {
                if data == "[DONE]" && protocol == RelayProtocol::ChatCompletions {
                    return Ok(
                        json!({"status":"ok","message":"流式请求通过（收到终止事件）","model":model}),
                    );
                }
                if let Ok(event) = serde_json::from_str::<Value>(data) {
                    ensure!(
                        event.get("error").is_none()
                            && !matches!(
                                event["type"].as_str(),
                                Some("response.failed" | "response.incomplete" | "error")
                            ),
                        "服务返回流式失败事件"
                    );
                    if event["type"] == "response.completed" && protocol == RelayProtocol::Responses
                    {
                        return Ok(
                            json!({"status":"ok","message":"流式请求通过（收到 response.completed）","model":model}),
                        );
                    }
                }
            }
        }
    }
    anyhow::bail!("测试流提前结束，未收到终止事件")
}

pub async fn call(service: &str, action: &str, request: Value) -> Result<Value, String> {
    async fn execute(service: &str, action: &str, request: Value) -> anyhow::Result<Value> {
        let input: Request = serde_json::from_value(request).context("服务参数格式不正确")?;
        let store = GatewayStore::default();
        let gateway = store.load()?;
        if action == "gatewayStatus" || (service == "workbuddy" && action == "status") {
            return Ok(summary(&gateway, service));
        }
        if matches!(action, "pauseGateway" | "enableGateway") {
            let current = provider(&gateway, service).context("尚未接入供应商池")?;
            let saved =
                apply::set_provider_enabled(&store, &current.relay.id, action == "enableGateway")?;
            return Ok(summary(&saved, service));
        }
        if service == "workbuddy" && action == "management" {
            let base = if input.base_url.is_empty() {
                provider(&gateway, service)
                    .map(|item| item.relay.upstream_base_url.as_str())
                    .unwrap_or_default()
            } else {
                &input.base_url
            };
            let url = workbuddy_url(base)?;
            let opened =
                crate::commands::open_external_url(url.trim_end_matches("/v1").to_string());
            ensure!(opened.status == "ok", "无法打开管理页：{}", opened.message);
            return Ok(json!({"status":"ok", "message":"已在浏览器打开完整 WorkBuddy Manager"}));
        }
        let (base, key) = connection(service, &input, &gateway)?;
        let models = models(&base, &key).await?;
        match action {
            "models" => Ok(json!({"status":"ok", "models":models, "baseUrl":base})),
            "applyGateway" | "apply" => upsert(&store, service, &base, &key, &models, &input),
            "testStream" => {
                ensure!(
                    models.iter().any(|model| model == &input.model),
                    "所选模型不在当前目录"
                );
                let protocol = input
                    .protocol
                    .or_else(|| provider(&gateway, service).map(|item| item.relay.protocol))
                    .unwrap_or(RelayProtocol::Responses);
                test_stream(&base, &key, &input.model, protocol).await
            }
            _ => anyhow::bail!("未开放的供应商操作"),
        }
    }
    execute(service, action, request)
        .await
        .map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 仅显式运维执行；密钥从本机既有存储读取，不放进命令行或测试输出。
    #[tokio::test]
    #[ignore = "连接真实本地服务并更新对应供应商；需显式指定服务"]
    async fn live_local_service_from_saved_credentials() {
        let service = std::env::var("CODEX_PLUS_VALIDATE_LOCAL_SERVICE").expect("需显式指定服务");
        assert!(matches!(
            service.as_str(),
            "workbuddy" | "cliproxy" | "newapi"
        ));
        let mut input = json!({});
        if service == "workbuddy" {
            let file =
                std::env::var("CODEX_PLUS_WORKBUDDY_KEY_FILE").expect("需指定本机既有密钥文件");
            input["apiKey"] = json!(std::fs::read_to_string(file).unwrap().trim());
        }
        let catalog = call(&service, "models", input.clone()).await.unwrap();
        input["model"] = catalog["models"][0].clone();
        let applied = call(&service, "applyGateway", input.clone()).await.unwrap();
        assert_eq!(applied["configured"], true);
        assert!(!applied["providerId"].as_str().unwrap().is_empty());
        let stream = call(&service, "testStream", input).await.unwrap();
        assert_eq!(stream["status"], "ok");
    }

    #[test]
    fn workbuddy_only_accepts_loopback_and_normalizes_ipv6() {
        assert_eq!(
            workbuddy_url("http://[::1]:7864/").unwrap(),
            "http://[::1]:7864/v1"
        );
        for bad in [
            "http://example.com/v1",
            "http://u:p@localhost",
            "http://localhost/v1?secret=x",
            "http://localhost/api",
            "file:///tmp",
        ] {
            assert!(workbuddy_url(bad).is_err());
        }
    }

    #[test]
    fn workbuddy_lane_rejects_unscoped_and_other_realm_keys_and_models() {
        let usage = |realm| json!({"object":"key.usage","key":{"realm":realm}});
        assert!(validate_lane_usage(&usage("cn"), "cn").is_ok());
        assert!(validate_lane_usage(&usage("global"), "global").is_ok());
        assert!(validate_lane_usage(&usage("global"), "cn").is_err());
        assert!(validate_lane_usage(&usage(""), "global").is_err());
        assert!(lane_model("cn:example", "cn"));
        assert!(lane_model("example", "cn"));
        assert!(!lane_model("example", "global"));
        assert!(!lane_model("global:example", "cn"));
    }

    #[test]
    fn updates_preserve_identity_paused_state_and_other_providers() {
        let temp = tempfile::tempdir().unwrap();
        let store = GatewayStore {
            path: temp.path().join("gateway.json"),
        };
        let models = vec!["model-a".into(), "model-b".into()];
        let first = upsert(
            &store,
            "workbuddy",
            "http://localhost:7864/v1",
            "test-key",
            &models,
            &Request::default(),
        )
        .unwrap();
        let id = first["providerId"].as_str().unwrap();
        apply::add_provider_key(&store, id, "spare-key").unwrap();
        store
            .update(|gateway| {
                let current = gateway
                    .providers
                    .iter_mut()
                    .find(|item| item.relay.id == id)
                    .unwrap();
                current.price = Some(codex_plus_core::gateway::store::ProviderPrice {
                    currency: "USD".into(),
                    input: 2.0,
                    cached_input: 1.0,
                    output: 3.0,
                });
                Ok(())
            })
            .unwrap();
        let original = store.load().unwrap().provider(id).unwrap().clone();
        apply::set_provider_enabled(&store, id, false).unwrap();
        upsert(
            &store,
            "newapi",
            "http://localhost:3000/v1",
            "other-key",
            &models,
            &Request::default(),
        )
        .unwrap();
        let second = upsert(
            &store,
            "workbuddy",
            "http://localhost:7864/v1",
            "new-key",
            &models,
            &Request::default(),
        )
        .unwrap();
        assert_eq!(first["providerId"], second["providerId"]);
        assert_eq!(second["enabled"], false);
        assert_eq!(store.load().unwrap().providers.len(), 2);
        let saved = store.load().unwrap().provider(id).unwrap().clone();
        assert_eq!(saved.price, original.price);
        assert_eq!(saved.relay.api_keys.len(), original.relay.api_keys.len());
        assert!(
            saved
                .relay
                .api_keys
                .iter()
                .any(|entry| entry.api_key == "spare-key")
        );
        assert_eq!(
            saved.relay.selected_api_key().unwrap().id,
            original.relay.selected_api_key().unwrap().id
        );
        assert!(!second.to_string().contains("test-key"));
    }

    #[test]
    fn reuses_legacy_import_and_rejects_address_change_without_new_key() {
        let temp = tempfile::tempdir().unwrap();
        let store = GatewayStore {
            path: temp.path().join("gateway.json"),
        };
        apply::create_provider(
            &store,
            &apply::ProviderInput {
                name: "custom name".into(),
                base_url: "http://localhost:3000/v1".into(),
                keys: vec!["test-key".into()],
                model: "model-a".into(),
                ..Default::default()
            },
            "codex:managed-newapi",
            None,
        )
        .unwrap();
        upsert(
            &store,
            "newapi",
            "http://localhost:3000/v1",
            "test-key",
            &["model-a".into()],
            &Request::default(),
        )
        .unwrap();
        assert_eq!(store.load().unwrap().providers.len(), 1);
        let mut gateway = store.load().unwrap();
        gateway.providers[0].source = "workbuddy".into();
        assert!(
            connection(
                "workbuddy",
                &Request {
                    base_url: "http://localhost:7864".into(),
                    ..Default::default()
                },
                &gateway
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn streaming_test_rejects_eof_failure_and_non_stream_responses() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (content_type, body, success) in [
            (
                "text/event-stream",
                "data: {\"type\":\"response.completed\"}\n\n",
                true,
            ),
            (
                "text/event-stream",
                "data: {\"type\":\"response.created\"}\n\n",
                false,
            ),
            (
                "text/event-stream",
                "data: {\"type\":\"response.failed\"}\n\n",
                false,
            ),
            ("application/json", "{}", false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                stream.read(&mut buf).await.unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            });
            let result = test_stream(
                &format!("http://{address}/v1"),
                "test-key",
                "test-model",
                RelayProtocol::Responses,
            )
            .await;
            assert_eq!(result.is_ok(), success, "{result:?}");
            server.await.unwrap();
        }
    }
}
