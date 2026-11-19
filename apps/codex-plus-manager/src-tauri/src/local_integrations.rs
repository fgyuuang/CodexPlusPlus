//! 管理器插件的限名服务适配；不开放任意命令，也不把原生凭据交给 iframe。
use crate::{cliproxy, newapi};
use codex_plus_core::user_scripts::UserScriptManager;
use serde::Serialize;
use serde_json::{Value, json};

fn public_result(result: impl Serialize) -> Result<Value, String> {
    let value = serde_json::to_value(result).map_err(|error| error.to_string())?;
    let mut public = serde_json::Map::new();
    // 明确投影：新加的原生字段默认不向插件开放。
    for field in [
        "status",
        "message",
        "configured",
        "installed",
        "running",
        "healthy",
        "managedProcess",
        "pid",
        "startedAt",
        "version",
        "installRoot",
        "binaryPath",
        "configPath",
        "baseUrl",
        "managementUrl",
        "autoStart",
        "profileInstalled",
        "officialProfileInstalled",
        "generalProfileInstalled",
        "dockerAvailable",
        "daemonAvailable",
        "composeAvailable",
        "systemName",
        "setup",
        "projectRoot",
        "composeFile",
        "dockerExecutable",
        "apiServiceName",
        "channelsUrl",
        "tokensUrl",
        "serviceCount",
        "runningServiceCount",
        "models",
        "endpoint",
        "httpStatus",
        "model",
        "profileId",
        "created",
    ] {
        if let Some(item) = value.get(field) {
            public.insert(field.into(), item.clone());
        }
    }
    for (secret, flag) in [
        ("apiKey", "hasApiKey"),
        ("managementKey", "hasManagementKey"),
    ] {
        if let Some(item) = value.get(secret).and_then(Value::as_str) {
            public.insert(flag.into(), json!(!item.is_empty()));
        }
    }
    Ok(Value::Object(public))
}

pub async fn call(
    manager: &UserScriptManager,
    key: &str,
    route: &str,
    payload: Value,
) -> Result<Value, String> {
    manager
        .authorize_local_integration(key, route)
        .map_err(|error| format!("{error:#}"))?;
    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .ok_or("缺少服务操作")?;
    let request = payload.get("request").cloned().unwrap_or_else(|| json!({}));
    let service = route
        .strip_prefix("/local-integrations/")
        .ok_or("未开放的服务")?;
    if matches!(
        action,
        "gatewayStatus" | "applyGateway" | "pauseGateway" | "enableGateway" | "testStream"
    ) || (service == "workbuddy"
        && matches!(action, "status" | "models" | "apply" | "management"))
    {
        let mut result = crate::local_provider_gateway::call(service, action, request).await?;
        if matches!(
            action,
            "applyGateway" | "apply" | "pauseGateway" | "enableGateway"
        ) {
            let refreshed = tauri::async_runtime::spawn_blocking(
                crate::gateway_commands::refresh_local_integration,
            )
            .await;
            if !matches!(refreshed, Ok(Ok(()))) {
                result["warning"] =
                    json!("供应商已保存，Agent 配置刷新未完成，请在 Agent 接入页重新连接");
            }
        }
        return Ok(result);
    }
    macro_rules! invoke {
        ($method:path) => {
            public_result($method().await)
        };
        ($method:path, $kind:ty) => {{
            let request: $kind =
                serde_json::from_value(request).map_err(|_| "服务参数格式不正确")?;
            public_result($method(request).await)
        }};
    }
    match (route, action) {
        ("/local-integrations/cliproxy", "status") => invoke!(cliproxy::cliproxy_status),
        ("/local-integrations/cliproxy", "install") => invoke!(cliproxy::cliproxy_install),
        ("/local-integrations/cliproxy", "start") => invoke!(cliproxy::cliproxy_start),
        ("/local-integrations/cliproxy", "stop") => invoke!(cliproxy::cliproxy_stop),
        ("/local-integrations/cliproxy", "restart") => invoke!(cliproxy::cliproxy_restart),
        ("/local-integrations/cliproxy", "management") => {
            invoke!(cliproxy::cliproxy_open_management)
        }
        ("/local-integrations/cliproxy", "models") => invoke!(cliproxy::cliproxy_list_models),
        ("/local-integrations/cliproxy", "test") => {
            invoke!(cliproxy::cliproxy_test_api, cliproxy::CliproxyTestRequest)
        }
        ("/local-integrations/cliproxy", "saveKey") => invoke!(
            cliproxy::cliproxy_save_api_key,
            cliproxy::CliproxySaveApiKeyRequest
        ),
        ("/local-integrations/cliproxy", "saveConnection") => invoke!(
            cliproxy::cliproxy_save_connection,
            cliproxy::CliproxySaveConnectionRequest
        ),
        ("/local-integrations/cliproxy", "autoStart") => invoke!(
            cliproxy::cliproxy_set_auto_start,
            cliproxy::CliproxySetAutoStartRequest
        ),
        ("/local-integrations/cliproxy", "apply") => invoke!(
            cliproxy::cliproxy_apply_profile,
            cliproxy::CliproxyApplyRequest
        ),
        ("/local-integrations/cliproxy", "disableOfficial") => {
            invoke!(cliproxy::cliproxy_disable_official_profile)
        }
        ("/local-integrations/cliproxy", "disable") => {
            invoke!(cliproxy::cliproxy_disable_integration)
        }
        ("/local-integrations/newapi", "status") => invoke!(newapi::newapi_status),
        ("/local-integrations/newapi", "start") => invoke!(newapi::newapi_start),
        ("/local-integrations/newapi", "stop") => invoke!(newapi::newapi_stop),
        ("/local-integrations/newapi", "restart") => invoke!(newapi::newapi_restart),
        ("/local-integrations/newapi", "management") => invoke!(newapi::newapi_open_management),
        ("/local-integrations/newapi", "channels") => invoke!(newapi::newapi_open_channels),
        ("/local-integrations/newapi", "tokens") => invoke!(newapi::newapi_open_tokens),
        ("/local-integrations/newapi", "models") => invoke!(newapi::newapi_list_models),
        ("/local-integrations/newapi", "test") => {
            invoke!(newapi::newapi_test_api, newapi::NewApiTestRequest)
        }
        ("/local-integrations/newapi", "saveKey") => {
            invoke!(newapi::newapi_save_api_key, newapi::NewApiSaveApiKeyRequest)
        }
        ("/local-integrations/newapi", "saveConnection") => invoke!(
            newapi::newapi_save_connection,
            newapi::NewApiSaveConnectionRequest
        ),
        ("/local-integrations/newapi", "apply") => {
            invoke!(newapi::newapi_apply_profile, newapi::NewApiApplyRequest)
        }
        ("/local-integrations/newapi", "disable") => invoke!(newapi::newapi_disable_integration),
        _ => Err("未开放的服务操作".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_excludes_credentials_settings_and_future_fields() {
        let result = public_result(json!({"status":"ok", "apiKey":"secret", "managementKey":"",
            "settings":{"relayProfiles":[{"apiKey":"other"}]}, "futureSecret":"hidden",
            "models":["example"], "profileId":"managed-newapi"}))
        .unwrap();
        assert_eq!(
            result,
            json!({"status":"ok", "hasApiKey":true, "hasManagementKey":false,
            "models":["example"], "profileId":"managed-newapi"})
        );
    }

    #[tokio::test]
    async fn dispatcher_rejects_undeclared_disabled_unknown_and_malformed_calls() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("user/panel");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("codexpp.json"), r#"{"id":"panel","manager":{"page":"panel.html"},"permissions":{"routes":["/local-integrations/cliproxy"]}}"#).unwrap();
        std::fs::write(root.join("panel.html"), "<p>panel</p>").unwrap();
        let manager = UserScriptManager::new(
            temp.path().join("builtin"),
            temp.path().join("user"),
            temp.path().join("scripts.json"),
        );
        for (route, payload) in [
            ("/local-integrations/newapi", json!({"action":"start"})),
            ("/local-integrations/cliproxy", json!({"action":"shell"})),
            ("/local-integrations/cliproxy", json!({})),
            (
                "/local-integrations/cliproxy",
                json!({"action":"saveConnection", "request":{"installRoot":42}}),
            ),
        ] {
            assert!(call(&manager, "user:panel", route, payload).await.is_err());
        }
        manager.set_script_enabled("user:panel", false).unwrap();
        assert!(
            call(
                &manager,
                "user:panel",
                "/local-integrations/cliproxy",
                json!({"action":"start"})
            )
            .await
            .is_err()
        );
        assert!(!temp.path().join("cliproxy-integration.json").exists());
    }
}
