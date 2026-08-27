use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use anyhow::{Context, ensure};
use base64::Engine as _;
use codex_plus_core::settings::{
    BackendSettings, RelayMode, RelayModelInsertMode, RelayProtocol, SettingsStore,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::CommandResult;

const DEFAULT_PROJECT_ROOT: &str = r"D:\pro\newapi";
const DEFAULT_DOCKER_EXECUTABLE: &str = "docker";
const DEFAULT_API_SERVICE_NAME: &str = "new-api";
const DEFAULT_SERVICE_URL: &str = "http://127.0.0.1:3000";
const MANAGED_PROFILE_ID: &str = "managed-newapi";
const INTEGRATION_TYPE: &str = "newapi";
const SECRET_BACKEND_DPAPI: &str = "windows-dpapi-current-user";
#[cfg(not(windows))]
const SECRET_BACKEND_USER_FILE: &str = "user-file";
const DPAPI_ENTROPY: &[u8] = b"CodexPlusPlus.NewAPI.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct NewApiIntegrationSettings {
    project_root: String,
    compose_file: String,
    docker_executable: String,
    api_service_name: String,
    base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_token: Option<SecretsEnvelope>,
}

impl Default for NewApiIntegrationSettings {
    fn default() -> Self {
        Self {
            project_root: DEFAULT_PROJECT_ROOT.to_string(),
            compose_file: default_compose_file(),
            docker_executable: DEFAULT_DOCKER_EXECUTABLE.to_string(),
            api_service_name: DEFAULT_API_SERVICE_NAME.to_string(),
            base_url: DEFAULT_SERVICE_URL.to_string(),
            api_token: None,
        }
    }
}

impl NewApiIntegrationSettings {
    fn api_key(&self) -> anyhow::Result<String> {
        let Some(envelope) = self.api_token.as_ref() else {
            return Ok(String::new());
        };
        let protected = base64::engine::general_purpose::STANDARD
            .decode(envelope.payload.as_bytes())
            .context("NewAPI API Token 凭据封装无效")?;
        let plaintext = unprotect_local_secret(&envelope.backend, &protected)?;
        let secrets: NewApiSecrets =
            serde_json::from_slice(&plaintext).context("NewAPI API Token 凭据内容无效")?;
        Ok(secrets.api_key)
    }

    fn service_root_url(&self) -> String {
        self.base_url.trim_end_matches('/').to_string()
    }

    fn api_base_url(&self) -> String {
        format!("{}/v1", self.service_root_url())
    }

    fn management_url(&self) -> String {
        self.service_root_url()
    }

    fn channels_url(&self) -> String {
        format!("{}/console/channel", self.service_root_url())
    }

    fn tokens_url(&self) -> String {
        format!("{}/console/token", self.service_root_url())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecretsEnvelope {
    version: u32,
    backend: String,
    payload: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewApiSecrets {
    version: u32,
    api_key: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewApiStatusPayload {
    pub configured: bool,
    pub docker_available: bool,
    pub daemon_available: bool,
    pub compose_available: bool,
    pub running: bool,
    pub healthy: bool,
    pub version: String,
    pub system_name: String,
    /// NewAPI `/api/status.data.start_time`, expressed as Unix seconds.
    pub started_at: Option<i64>,
    pub setup: bool,
    pub project_root: String,
    pub compose_file: String,
    pub docker_executable: String,
    pub api_service_name: String,
    pub base_url: String,
    pub management_url: String,
    pub channels_url: String,
    pub tokens_url: String,
    pub api_key: String,
    pub profile_installed: bool,
    pub service_count: usize,
    pub running_service_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewApiModelsPayload {
    pub models: Vec<String>,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewApiTestPayload {
    pub http_status: u16,
    pub endpoint: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewApiApplyPayload {
    pub settings: BackendSettings,
    pub profile_id: String,
    pub created: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewApiSaveConnectionRequest {
    pub project_root: String,
    #[serde(default)]
    pub compose_file: String,
    #[serde(default)]
    pub docker_executable: String,
    #[serde(default)]
    pub api_service_name: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewApiSaveApiKeyRequest {
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewApiTestRequest {
    #[serde(default)]
    pub model: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewApiApplyRequest {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Debug, Default)]
struct DockerStatus {
    docker_available: bool,
    daemon_available: bool,
    compose_available: bool,
    running: bool,
    service_count: usize,
    running_service_count: usize,
}

#[derive(Debug, Default)]
struct PublicStatusInfo {
    version: String,
    system_name: String,
    started_at: Option<i64>,
    setup: bool,
}

#[tauri::command]
pub async fn newapi_status() -> CommandResult<NewApiStatusPayload> {
    match status_payload().await {
        Ok(payload) => success("NewAPI 状态已刷新。", payload),
        Err(error) => failure(&format!("读取 NewAPI 状态失败：{error}"), fallback_status()),
    }
}

#[tauri::command]
pub async fn newapi_start() -> CommandResult<NewApiStatusPayload> {
    match run_compose_action(&["up", "-d"]).await {
        Ok(()) => {
            wait_for_health().await;
            success(
                "NewAPI Compose 服务已启动。",
                status_payload().await.unwrap_or_else(|_| fallback_status()),
            )
        }
        Err(error) => failure(
            &format!("启动 NewAPI 失败：{error}"),
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
    }
}

#[tauri::command]
pub async fn newapi_stop() -> CommandResult<NewApiStatusPayload> {
    match run_compose_action(&["stop"]).await {
        Ok(()) => {
            tokio::time::sleep(Duration::from_millis(500)).await;
            success(
                "NewAPI Compose 服务已停止。",
                status_payload().await.unwrap_or_else(|_| fallback_status()),
            )
        }
        Err(error) => failure(
            &format!("停止 NewAPI 失败：{error}"),
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
    }
}

#[tauri::command]
pub async fn newapi_restart() -> CommandResult<NewApiStatusPayload> {
    match run_compose_action(&["restart"]).await {
        Ok(()) => {
            wait_for_health().await;
            success(
                "NewAPI Compose 服务已重启。",
                status_payload().await.unwrap_or_else(|_| fallback_status()),
            )
        }
        Err(error) => failure(
            &format!("重启 NewAPI 失败：{error}"),
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
    }
}

#[tauri::command]
pub async fn newapi_open_management() -> CommandResult<Value> {
    open_configured_url(|settings| settings.management_url())
}

#[tauri::command]
pub async fn newapi_open_channels() -> CommandResult<Value> {
    open_configured_url(|settings| settings.channels_url())
}

#[tauri::command]
pub async fn newapi_open_tokens() -> CommandResult<Value> {
    open_configured_url(|settings| settings.tokens_url())
}

#[tauri::command]
pub async fn newapi_list_models() -> CommandResult<NewApiModelsPayload> {
    match list_models_payload().await {
        Ok(payload) => success(
            &format!("NewAPI 返回了 {} 个模型。", payload.models.len()),
            payload,
        ),
        Err(error) => failure(
            &format!("读取 NewAPI 模型失败：{error}"),
            NewApiModelsPayload {
                models: Vec::new(),
                endpoint: String::new(),
            },
        ),
    }
}

#[tauri::command]
pub async fn newapi_test_api(request: NewApiTestRequest) -> CommandResult<NewApiTestPayload> {
    match test_api(&request.model).await {
        Ok(payload) => success(
            &format!(
                "NewAPI 请求成功，模型「{}」，HTTP {}。",
                payload.model, payload.http_status
            ),
            payload,
        ),
        Err(error) => failure(
            &format!("测试 NewAPI 失败：{error}"),
            NewApiTestPayload {
                http_status: 0,
                endpoint: String::new(),
                model: request.model,
            },
        ),
    }
}

#[tauri::command]
pub async fn newapi_save_connection(
    request: NewApiSaveConnectionRequest,
) -> CommandResult<NewApiStatusPayload> {
    match save_connection_settings(request) {
        Ok(()) => match status_payload().await {
            Ok(payload) => success("NewAPI 启动与连接设置已保存。", payload),
            Err(error) => failure(
                &format!("NewAPI 连接设置已保存，但刷新状态失败：{error}"),
                fallback_status(),
            ),
        },
        Err(error) => failure(
            &format!("保存 NewAPI 启动与连接设置失败：{error}"),
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
    }
}

#[tauri::command]
pub async fn newapi_save_api_key(
    request: NewApiSaveApiKeyRequest,
) -> CommandResult<NewApiStatusPayload> {
    let api_key = request.api_key.trim();
    if api_key.is_empty() {
        return failure("NewAPI API Token 不能为空。", fallback_status());
    }
    match save_api_key(api_key) {
        Ok(()) => success(
            "NewAPI API Token 已使用当前 Windows 用户的 DPAPI 凭据保存。",
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
        Err(error) => failure(
            &format!("保存 NewAPI API Token 失败：{error}"),
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
    }
}

#[tauri::command]
pub async fn newapi_apply_profile(
    mut request: NewApiApplyRequest,
) -> CommandResult<NewApiApplyPayload> {
    if request.models.is_empty() {
        match list_models_payload().await {
            Ok(payload) => request.models = payload.models,
            Err(error) => {
                return failure(
                    &format!("应用 NewAPI 供应商失败：{error}"),
                    fallback_apply_payload(),
                );
            }
        }
    }
    match apply_profile(request) {
        Ok(payload) => success(
            "NewAPI 已作为普通 Responses 供应商接入，可按现有规则独立使用或加入聚合。",
            payload,
        ),
        Err(error) => failure(
            &format!("应用 NewAPI 供应商失败：{error}"),
            fallback_apply_payload(),
        ),
    }
}

#[tauri::command]
pub async fn newapi_disable_integration() -> CommandResult<NewApiApplyPayload> {
    match disable_integration() {
        Ok(payload) => success(
            "NewAPI 供应商接入已关闭；Compose 服务、渠道和令牌数据未受影响。",
            payload,
        ),
        Err(error) => failure(
            &format!("关闭 NewAPI 供应商接入失败：{error}"),
            fallback_apply_payload(),
        ),
    }
}

fn default_compose_file() -> String {
    PathBuf::from(DEFAULT_PROJECT_ROOT)
        .join("docker-compose.yml")
        .to_string_lossy()
        .to_string()
}

fn integration_settings_path() -> PathBuf {
    codex_plus_core::paths::default_settings_path().with_file_name("newapi-integration.json")
}

fn load_integration_settings() -> anyhow::Result<NewApiIntegrationSettings> {
    load_integration_settings_from(&integration_settings_path())
}

fn load_integration_settings_from(path: &Path) -> anyhow::Result<NewApiIntegrationSettings> {
    let settings = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("NewAPI 连接设置格式无效：{}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            NewApiIntegrationSettings::default()
        }
        Err(error) => return Err(error.into()),
    };
    normalize_integration_settings(settings)
}

fn normalize_integration_settings(
    mut settings: NewApiIntegrationSettings,
) -> anyhow::Result<NewApiIntegrationSettings> {
    settings.project_root = absolute_path_text(&settings.project_root, "项目目录")?;
    settings.compose_file = compose_path_text(&settings.project_root, &settings.compose_file)?;
    settings.docker_executable = docker_executable_text(&settings.docker_executable)?;
    settings.api_service_name = service_name_text(&settings.api_service_name)?;
    settings.base_url = normalize_service_url(&settings.base_url)?;
    Ok(settings)
}

fn save_connection_settings(request: NewApiSaveConnectionRequest) -> anyhow::Result<()> {
    save_connection_settings_to(&integration_settings_path(), request)
}

fn save_connection_settings_to(
    path: &Path,
    request: NewApiSaveConnectionRequest,
) -> anyhow::Result<()> {
    let current = load_integration_settings_from(path)?;
    let next = normalize_integration_settings(NewApiIntegrationSettings {
        project_root: request.project_root,
        compose_file: request.compose_file,
        docker_executable: request.docker_executable,
        api_service_name: request.api_service_name,
        base_url: request.base_url,
        api_token: current.api_token,
    })?;
    atomic_write_json(path, &next)
}

fn save_api_key(api_key: &str) -> anyhow::Result<()> {
    save_api_key_to(&integration_settings_path(), api_key)
}

fn save_api_key_to(path: &Path, api_key: &str) -> anyhow::Result<()> {
    let api_key = api_key.trim();
    ensure!(!api_key.is_empty(), "NewAPI API Token 不能为空");
    let mut settings = load_integration_settings_from(path)?;
    let plaintext = serde_json::to_vec(&NewApiSecrets {
        version: 1,
        api_key: api_key.to_string(),
    })?;
    let (backend, protected) = protect_local_secret(&plaintext)?;
    settings.api_token = Some(SecretsEnvelope {
        version: 1,
        backend,
        payload: base64::engine::general_purpose::STANDARD.encode(protected),
    });
    atomic_write_json(path, &settings)
}

fn absolute_path_text(value: &str, label: &str) -> anyhow::Result<String> {
    let value = value.trim();
    ensure!(!value.is_empty(), "{label}不能为空");
    let path = PathBuf::from(value);
    ensure!(path.is_absolute(), "{label}必须是绝对路径");
    Ok(path.to_string_lossy().to_string())
}

fn compose_path_text(project_root: &str, value: &str) -> anyhow::Result<String> {
    let root = PathBuf::from(project_root);
    let value = value.trim();
    let path = if value.is_empty() {
        root.join("docker-compose.yml")
    } else {
        let configured = PathBuf::from(value);
        if configured.is_absolute() {
            configured
        } else {
            root.join(configured)
        }
    };
    ensure!(path.is_absolute(), "Compose 文件必须解析为绝对路径");
    Ok(path.to_string_lossy().to_string())
}

fn docker_executable_text(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    let value = if value.is_empty() {
        DEFAULT_DOCKER_EXECUTABLE
    } else {
        value
    };
    let path = Path::new(value);
    ensure!(
        path.is_absolute() || path.components().count() == 1,
        "Docker 可执行文件必须是 PATH 中的命令名或绝对路径"
    );
    Ok(value.to_string())
}

fn service_name_text(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    let value = if value.is_empty() {
        DEFAULT_API_SERVICE_NAME
    } else {
        value
    };
    ensure!(
        !value.starts_with('-')
            && value
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character)),
        "API 服务名只能包含字母、数字、下划线、点和连字符"
    );
    Ok(value.to_string())
}

fn normalize_service_url(base_url: &str) -> anyhow::Result<String> {
    let base_url = base_url.trim().trim_end_matches('/');
    ensure!(!base_url.is_empty(), "API Base URL 不能为空");
    let parsed = reqwest::Url::parse(base_url).context("API Base URL 格式无效")?;
    ensure!(
        parsed.scheme() == "http" || parsed.scheme() == "https",
        "API Base URL 仅支持 http 或 https"
    );
    ensure!(
        parsed.username().is_empty() && parsed.password().is_none(),
        "API Base URL 不得包含用户名或密码"
    );
    ensure!(
        parsed.query().is_none() && parsed.fragment().is_none(),
        "API Base URL 不得包含查询参数或片段"
    );
    Ok(base_url
        .strip_suffix("/v1")
        .unwrap_or(base_url)
        .trim_end_matches('/')
        .to_string())
}

fn compose_args(settings: &NewApiIntegrationSettings, tail: &[&str]) -> Vec<String> {
    let mut args = vec![
        "compose".to_string(),
        "--project-directory".to_string(),
        settings.project_root.clone(),
        "-f".to_string(),
        settings.compose_file.clone(),
    ];
    args.extend(tail.iter().map(|value| (*value).to_string()));
    args
}

async fn run_process(
    settings: &NewApiIntegrationSettings,
    args: &[String],
    use_project_directory: bool,
    timeout: Duration,
) -> anyhow::Result<Output> {
    let mut command = tokio::process::Command::new(&settings.docker_executable);
    command.args(args);
    if use_project_directory {
        command.current_dir(&settings.project_root);
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    command.kill_on_drop(true);
    tokio::time::timeout(timeout, command.output())
        .await
        .context("Docker 命令执行超时")?
        .context("无法启动 Docker CLI")
}

async fn run_compose_action(action: &[&str]) -> anyhow::Result<()> {
    ensure!(
        matches!(action, ["up", "-d"] | ["stop"] | ["restart"]),
        "不支持的 Compose 操作"
    );
    let settings = load_integration_settings()?;
    validate_compose_layout(&settings)?;
    let args = compose_args(&settings, action);
    ensure!(
        !args.iter().any(|arg| arg == "down"),
        "禁止执行 compose down"
    );
    let output = run_process(&settings, &args, true, Duration::from_secs(300)).await?;
    ensure!(
        output.status.success(),
        "Docker Compose 命令失败（退出码 {}）",
        output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );
    Ok(())
}

fn validate_compose_layout(settings: &NewApiIntegrationSettings) -> anyhow::Result<()> {
    ensure!(
        Path::new(&settings.project_root).is_dir(),
        "NewAPI 项目目录不存在"
    );
    ensure!(
        Path::new(&settings.compose_file).is_file(),
        "Docker Compose 文件不存在"
    );
    Ok(())
}

async fn inspect_docker(settings: &NewApiIntegrationSettings) -> DockerStatus {
    let docker_available = command_succeeds(
        settings,
        &["--version".to_string()],
        false,
        Duration::from_secs(5),
    )
    .await;
    if !docker_available {
        return DockerStatus::default();
    }

    let daemon_available = command_succeeds(
        settings,
        &[
            "info".to_string(),
            "--format".to_string(),
            "{{.ServerVersion}}".to_string(),
        ],
        false,
        Duration::from_secs(10),
    )
    .await;
    let compose_available = command_succeeds(
        settings,
        &["compose".to_string(), "version".to_string()],
        false,
        Duration::from_secs(10),
    )
    .await;

    let mut service_names = Vec::new();
    let mut running_service_names = Vec::new();
    if compose_available && validate_compose_layout(settings).is_ok() {
        let config_args = compose_args(settings, &["config", "--services"]);
        if let Ok(output) = run_process(settings, &config_args, true, Duration::from_secs(15)).await
        {
            if output.status.success() {
                service_names = output_lines(&output.stdout);
            }
        }
        if daemon_available {
            let ps_args = compose_args(settings, &["ps", "--services", "--status", "running"]);
            if let Ok(output) = run_process(settings, &ps_args, true, Duration::from_secs(15)).await
            {
                if output.status.success() {
                    running_service_names = output_lines(&output.stdout);
                }
            }
        }
    }
    deduplicate(&mut service_names);
    deduplicate(&mut running_service_names);
    let running = running_service_names
        .iter()
        .any(|name| name == &settings.api_service_name);
    DockerStatus {
        docker_available,
        daemon_available,
        compose_available,
        running,
        service_count: service_names.len().max(running_service_names.len()),
        running_service_count: running_service_names.len(),
    }
}

async fn command_succeeds(
    settings: &NewApiIntegrationSettings,
    args: &[String],
    use_project_directory: bool,
    timeout: Duration,
) -> bool {
    run_process(settings, args, use_project_directory, timeout)
        .await
        .is_ok_and(|output| output.status.success())
}

fn output_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn deduplicate(values: &mut Vec<String>) {
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

async fn status_payload() -> anyhow::Result<NewApiStatusPayload> {
    let settings = load_integration_settings()?;
    let docker = inspect_docker(&settings).await;
    let public_status = fetch_public_status(&settings).await.ok();
    let backend_settings = SettingsStore::default().load().unwrap_or_default();
    Ok(NewApiStatusPayload {
        configured: Path::new(&settings.project_root).is_dir()
            && Path::new(&settings.compose_file).is_file(),
        docker_available: docker.docker_available,
        daemon_available: docker.daemon_available,
        compose_available: docker.compose_available,
        running: docker.running,
        healthy: public_status.is_some(),
        version: public_status
            .as_ref()
            .map(|status| status.version.clone())
            .unwrap_or_default(),
        system_name: public_status
            .as_ref()
            .map(|status| status.system_name.clone())
            .unwrap_or_default(),
        started_at: public_status.as_ref().and_then(|status| status.started_at),
        setup: public_status.as_ref().is_some_and(|status| status.setup),
        project_root: settings.project_root.clone(),
        compose_file: settings.compose_file.clone(),
        docker_executable: settings.docker_executable.clone(),
        api_service_name: settings.api_service_name.clone(),
        base_url: settings.api_base_url(),
        management_url: settings.management_url(),
        channels_url: settings.channels_url(),
        tokens_url: settings.tokens_url(),
        api_key: settings.api_key()?,
        profile_installed: backend_settings
            .relay_profiles
            .iter()
            .any(is_managed_profile),
        service_count: docker.service_count,
        running_service_count: docker.running_service_count,
    })
}

async fn fetch_public_status(
    settings: &NewApiIntegrationSettings,
) -> anyhow::Result<PublicStatusInfo> {
    let endpoint = format!("{}/api/status", settings.service_root_url());
    let response = http_client(Duration::from_secs(4))?
        .get(endpoint)
        .send()
        .await
        .context("连接 /api/status 失败")?;
    let status = response.status();
    ensure!(
        status.is_success(),
        "/api/status 返回 HTTP {}",
        status.as_u16()
    );
    let body: Value = response
        .json()
        .await
        .context("/api/status 响应不是有效 JSON")?;
    parse_public_status(&body)
}

fn parse_public_status(body: &Value) -> anyhow::Result<PublicStatusInfo> {
    ensure!(
        body.get("success").and_then(Value::as_bool) == Some(true),
        "/api/status 未报告成功状态"
    );
    let data = body
        .get("data")
        .and_then(Value::as_object)
        .context("/api/status 的 data 字段不是对象")?;
    let started_at = integer_value(data.get("start_time"))
        .context("/api/status 的 start_time 字段不是有效整数")?;
    let setup = data
        .get("setup")
        .and_then(Value::as_bool)
        .context("/api/status 的 setup 字段不是布尔值")?;
    Ok(PublicStatusInfo {
        version: scalar_string(data.get("version")),
        system_name: scalar_string(data.get("system_name")),
        started_at: Some(started_at),
        setup,
    })
}

fn scalar_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.trim().to_string(),
        Some(Value::Number(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn integer_value(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
    })
}

async fn wait_for_health() {
    let Ok(settings) = load_integration_settings() else {
        return;
    };
    for _ in 0..20 {
        if fetch_public_status(&settings).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn open_configured_url(
    select_url: impl FnOnce(&NewApiIntegrationSettings) -> String,
) -> CommandResult<Value> {
    let settings = load_integration_settings().unwrap_or_default();
    crate::commands::open_external_url(select_url(&settings))
}

async fn list_models_payload() -> anyhow::Result<NewApiModelsPayload> {
    let settings = load_integration_settings()?;
    let api_key = settings.api_key()?;
    ensure!(!api_key.trim().is_empty(), "请先保存 NewAPI API Token");
    let endpoint = format!("{}/models", settings.api_base_url());
    let response = http_client(Duration::from_secs(30))?
        .get(&endpoint)
        .bearer_auth(api_key)
        .send()
        .await
        .context("连接 /v1/models 失败")?;
    let status = response.status();
    ensure!(
        status.is_success(),
        "/v1/models 返回 HTTP {}",
        status.as_u16()
    );
    let body: Value = response.json().await.context("模型响应不是有效 JSON")?;
    let mut models = body
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    deduplicate(&mut models);
    Ok(NewApiModelsPayload { models, endpoint })
}

async fn test_api(requested_model: &str) -> anyhow::Result<NewApiTestPayload> {
    let models = list_models_payload().await?;
    let model = if requested_model.trim().is_empty() {
        models
            .models
            .first()
            .cloned()
            .context("NewAPI 未返回可测试模型")?
    } else {
        requested_model.trim().to_string()
    };
    ensure!(
        models.models.iter().any(|candidate| candidate == &model),
        "测试模型不在 NewAPI 模型列表中"
    );
    let settings = load_integration_settings()?;
    let api_key = settings.api_key()?;
    ensure!(!api_key.trim().is_empty(), "请先保存 NewAPI API Token");
    let endpoint = format!("{}/responses", settings.api_base_url());
    let response = http_client(Duration::from_secs(90))?
        .post(&endpoint)
        .bearer_auth(api_key)
        .json(&json!({
            "model": model,
            "input": "Respond with OK.",
            "stream": false,
        }))
        .send()
        .await
        .context("连接 /v1/responses 失败")?;
    let status = response.status();
    ensure!(
        status.is_success(),
        "/v1/responses 返回 HTTP {}",
        status.as_u16()
    );
    Ok(NewApiTestPayload {
        http_status: status.as_u16(),
        endpoint,
        model,
    })
}

fn http_client(timeout: Duration) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .context("创建 NewAPI HTTP 客户端失败")
}

fn apply_profile(request: NewApiApplyRequest) -> anyhow::Result<NewApiApplyPayload> {
    let integration = load_integration_settings()?;
    let api_key = integration.api_key()?;
    let store = SettingsStore::default();
    apply_profile_with(request, &integration, &api_key, &store)
}

fn apply_profile_with(
    request: NewApiApplyRequest,
    integration: &NewApiIntegrationSettings,
    api_key: &str,
    store: &SettingsStore,
) -> anyhow::Result<NewApiApplyPayload> {
    ensure!(!api_key.trim().is_empty(), "NewAPI API Token 不能为空");
    let mut models = request
        .models
        .into_iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    deduplicate(&mut models);
    ensure!(!models.is_empty(), "NewAPI 未返回可用模型");

    let requested_model = request.model.trim();
    let model = models
        .iter()
        .find(|candidate| candidate.as_str() == requested_model)
        .cloned()
        .unwrap_or_else(|| models[0].clone());
    let mut settings = store.load().unwrap_or_default();
    let existing_index = settings.relay_profiles.iter().position(is_managed_profile);
    let created = existing_index.is_none();
    let previous_id = existing_index
        .and_then(|index| settings.relay_profiles.get(index))
        .map(|profile| profile.id.clone());
    let mut profile = existing_index
        .and_then(|index| settings.relay_profiles.get(index).cloned())
        .unwrap_or_default();
    profile.id = MANAGED_PROFILE_ID.to_string();
    profile.name = "NewAPI".to_string();
    profile.integration_type = INTEGRATION_TYPE.to_string();
    profile.model = model.clone();
    profile.base_url = integration.api_base_url();
    profile.upstream_base_url = integration.api_base_url();
    profile.api_key = api_key.trim().to_string();
    profile.protocol = RelayProtocol::Responses;
    profile.relay_mode = RelayMode::PureApi;
    profile.official_mix_api_key = false;
    profile.test_model = model;
    profile.config_contents.clear();
    profile.auth_contents = serde_json::to_string_pretty(&json!({
        "OPENAI_API_KEY": api_key.trim(),
    }))?;
    profile.use_common_config = true;
    profile.model_insert_mode = RelayModelInsertMode::Patch;
    profile.model_list = models.join("\n");
    profile.model_mappings.clear();
    profile.model_mappings_enabled = true;
    codex_plus_core::relay_config::normalize_relay_profile_for_storage(&mut profile)?;
    if let Some(index) = existing_index {
        settings.relay_profiles[index] = profile;
    } else {
        settings.relay_profiles.push(profile);
    }
    if let Some(previous_id) = previous_id.filter(|id| id != MANAGED_PROFILE_ID) {
        rewrite_relay_references(&mut settings, &previous_id, MANAGED_PROFILE_ID);
    }
    store.save(&settings)?;
    Ok(NewApiApplyPayload {
        settings,
        profile_id: MANAGED_PROFILE_ID.to_string(),
        created,
    })
}

fn rewrite_relay_references(settings: &mut BackendSettings, from: &str, to: &str) {
    if settings.active_relay_id == from {
        settings.active_relay_id = to.to_string();
    }
    if settings.official_login_relay_id == from {
        settings.official_login_relay_id = to.to_string();
    }
    for aggregate in &mut settings.aggregate_relay_profiles {
        for member in &mut aggregate.members {
            if member.relay_id == from {
                member.relay_id = to.to_string();
            }
        }
        for mapping in &mut aggregate.model_mappings {
            for target in &mut mapping.targets {
                if target.relay_id == from {
                    target.relay_id = to.to_string();
                }
            }
        }
    }
}

fn disable_integration() -> anyhow::Result<NewApiApplyPayload> {
    let store = SettingsStore::default();
    disable_integration_with(&store)
}

fn disable_integration_with(store: &SettingsStore) -> anyhow::Result<NewApiApplyPayload> {
    let mut settings = store.load().unwrap_or_default();
    let mut removed_ids = settings
        .relay_profiles
        .iter()
        .filter(|profile| is_managed_profile(profile))
        .map(|profile| profile.id.clone())
        .collect::<HashSet<_>>();
    removed_ids.insert(MANAGED_PROFILE_ID.to_string());
    settings
        .relay_profiles
        .retain(|profile| !is_managed_profile(profile));

    let mut emptied_aggregates = HashSet::new();
    for aggregate in &mut settings.aggregate_relay_profiles {
        let had_managed_member = aggregate
            .members
            .iter()
            .any(|member| removed_ids.contains(&member.relay_id));
        aggregate
            .members
            .retain(|member| !removed_ids.contains(&member.relay_id));
        for mapping in &mut aggregate.model_mappings {
            mapping
                .targets
                .retain(|target| !removed_ids.contains(&target.relay_id));
        }
        aggregate
            .model_mappings
            .retain(|mapping| !mapping.targets.is_empty());
        if had_managed_member && aggregate.members.is_empty() {
            emptied_aggregates.insert(aggregate.id.clone());
        }
    }

    if removed_ids.contains(&settings.official_login_relay_id) {
        settings.official_login_relay_id.clear();
    }
    if removed_ids.contains(&settings.active_aggregate_relay_id)
        || emptied_aggregates.contains(&settings.active_aggregate_relay_id)
    {
        settings.active_aggregate_relay_id.clear();
    }
    if removed_ids.contains(&settings.active_relay_id)
        || emptied_aggregates.contains(&settings.active_relay_id)
    {
        settings.active_relay_id = fallback_relay_id(&settings);
    }
    store.save(&settings)?;
    Ok(NewApiApplyPayload {
        settings,
        profile_id: MANAGED_PROFILE_ID.to_string(),
        created: false,
    })
}

fn fallback_relay_id(settings: &BackendSettings) -> String {
    if !settings.active_aggregate_relay_id.trim().is_empty()
        && settings.aggregate_relay_profiles.iter().any(|aggregate| {
            aggregate.id == settings.active_aggregate_relay_id && !aggregate.members.is_empty()
        })
        && settings
            .relay_profiles
            .iter()
            .any(|profile| profile.id == settings.active_aggregate_relay_id)
    {
        return settings.active_aggregate_relay_id.clone();
    }
    settings
        .relay_profiles
        .iter()
        .find(|profile| {
            settings
                .aggregate_relay_profiles
                .iter()
                .find(|aggregate| aggregate.id == profile.id)
                .is_none_or(|aggregate| !aggregate.members.is_empty())
        })
        .map(|profile| profile.id.clone())
        .unwrap_or_else(|| "default".to_string())
}

fn is_managed_profile(profile: &codex_plus_core::settings::RelayProfile) -> bool {
    profile.id == MANAGED_PROFILE_ID || profile.integration_type == INTEGRATION_TYPE
}

fn fallback_status() -> NewApiStatusPayload {
    let settings = load_integration_settings().unwrap_or_default();
    let backend_settings = SettingsStore::default().load().unwrap_or_default();
    NewApiStatusPayload {
        configured: Path::new(&settings.project_root).is_dir()
            && Path::new(&settings.compose_file).is_file(),
        docker_available: false,
        daemon_available: false,
        compose_available: false,
        running: false,
        healthy: false,
        version: String::new(),
        system_name: String::new(),
        started_at: None,
        setup: false,
        project_root: settings.project_root.clone(),
        compose_file: settings.compose_file.clone(),
        docker_executable: settings.docker_executable.clone(),
        api_service_name: settings.api_service_name.clone(),
        base_url: settings.api_base_url(),
        management_url: settings.management_url(),
        channels_url: settings.channels_url(),
        tokens_url: settings.tokens_url(),
        api_key: settings.api_key().unwrap_or_default(),
        profile_installed: backend_settings
            .relay_profiles
            .iter()
            .any(is_managed_profile),
        service_count: 0,
        running_service_count: 0,
    }
}

fn fallback_apply_payload() -> NewApiApplyPayload {
    NewApiApplyPayload {
        settings: SettingsStore::default().load().unwrap_or_default(),
        profile_id: MANAGED_PROFILE_ID.to_string(),
        created: false,
    }
}

fn atomic_write_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    codex_plus_core::settings::atomic_write(path, &bytes)
}

fn success<T: Serialize>(message: &str, payload: T) -> CommandResult<T> {
    CommandResult {
        status: "ok".to_string(),
        message: message.to_string(),
        payload,
    }
}

fn failure<T: Serialize>(message: &str, payload: T) -> CommandResult<T> {
    CommandResult {
        status: "failed".to_string(),
        message: message.to_string(),
        payload,
    }
}

#[cfg(windows)]
fn protect_local_secret(bytes: &[u8]) -> anyhow::Result<(String, Vec<u8>)> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    };
    use windows::core::PCWSTR;

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).context("NewAPI API Token 凭据过大")?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(DPAPI_ENTROPY.len()).unwrap_or_default(),
        pbData: DPAPI_ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        let protected = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData.cast()));
        Ok((SECRET_BACKEND_DPAPI.to_string(), protected))
    }
}

#[cfg(windows)]
fn unprotect_local_secret(backend: &str, bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };

    ensure!(backend == SECRET_BACKEND_DPAPI, "不支持的凭据保护后端");
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).context("NewAPI API Token 凭据过大")?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(DPAPI_ENTROPY.len()).unwrap_or_default(),
        pbData: DPAPI_ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        let plaintext = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData.cast()));
        Ok(plaintext)
    }
}

#[cfg(not(windows))]
fn protect_local_secret(bytes: &[u8]) -> anyhow::Result<(String, Vec<u8>)> {
    Ok((SECRET_BACKEND_USER_FILE.to_string(), bytes.to_vec()))
}

#[cfg(not(windows))]
fn unprotect_local_secret(backend: &str, bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    ensure!(backend == SECRET_BACKEND_USER_FILE, "不支持的凭据保护后端");
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_plus_core::settings::{
        AggregateRelayDispatchTarget, AggregateRelayMember, AggregateRelayModelMapping,
        AggregateRelayProfile, AggregateRelayStrategy, RelayProfile, RelaySessionProvider,
    };

    fn temp_integration(root: &Path) -> NewApiIntegrationSettings {
        normalize_integration_settings(NewApiIntegrationSettings {
            project_root: root.to_string_lossy().to_string(),
            compose_file: root.join("compose.yaml").to_string_lossy().to_string(),
            docker_executable: "docker".to_string(),
            api_service_name: "api".to_string(),
            base_url: "http://127.0.0.1:3000/v1/".to_string(),
            api_token: None,
        })
        .unwrap()
    }

    #[test]
    fn url_normalization_keeps_service_root_and_builds_api_routes() {
        let temp = tempfile::tempdir().unwrap();
        let integration = temp_integration(temp.path());
        assert_eq!(integration.base_url, "http://127.0.0.1:3000");
        assert_eq!(integration.api_base_url(), "http://127.0.0.1:3000/v1");
        assert_eq!(
            integration.channels_url(),
            "http://127.0.0.1:3000/console/channel"
        );
        assert!(normalize_service_url("ftp://127.0.0.1:3000").is_err());
        assert!(normalize_service_url("http://user:pass@127.0.0.1:3000").is_err());
        assert!(normalize_service_url("http://127.0.0.1:3000?token=x").is_err());
    }

    #[test]
    fn public_status_requires_newapi_shape_fields() {
        let valid = json!({
            "success": true,
            "data": {
                "version": "v0.9.0",
                "start_time": 1_725_000_000,
                "system_name": "New API",
                "setup": true,
            }
        });
        let parsed = parse_public_status(&valid).unwrap();
        assert_eq!(parsed.started_at, Some(1_725_000_000));
        assert!(parsed.setup);

        let missing_start = json!({
            "success": true,
            "data": { "setup": true }
        });
        assert!(parse_public_status(&missing_start).is_err());
        let invalid_setup = json!({
            "success": true,
            "data": { "start_time": 1, "setup": "yes" }
        });
        assert!(parse_public_status(&invalid_setup).is_err());
        let invalid_data = json!({ "success": true, "data": [] });
        assert!(parse_public_status(&invalid_data).is_err());
    }

    #[test]
    fn connection_paths_are_explicit_and_relative_compose_is_rooted() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("NewAPI project");
        let settings = normalize_integration_settings(NewApiIntegrationSettings {
            project_root: root.to_string_lossy().to_string(),
            compose_file: "deploy/compose.yml".to_string(),
            docker_executable: "docker.exe".to_string(),
            api_service_name: "new-api_2".to_string(),
            base_url: DEFAULT_SERVICE_URL.to_string(),
            api_token: None,
        })
        .unwrap();
        assert_eq!(
            PathBuf::from(settings.compose_file),
            root.join("deploy").join("compose.yml")
        );
        assert!(absolute_path_text("relative", "项目目录").is_err());
        assert!(docker_executable_text("tools/docker.exe").is_err());
        assert!(service_name_text("--project-directory").is_err());
    }

    #[test]
    fn compose_arguments_are_parameterized_and_never_use_down() {
        let temp = tempfile::tempdir().unwrap();
        let integration = temp_integration(temp.path());
        let up = compose_args(&integration, &["up", "-d"]);
        assert_eq!(up[0], "compose");
        assert_eq!(up[1], "--project-directory");
        assert_eq!(up[2], integration.project_root);
        assert_eq!(up[3], "-f");
        assert_eq!(up[4], integration.compose_file);
        assert_eq!(&up[5..], ["up", "-d"]);
        let stop = compose_args(&integration, &["stop"]);
        let restart = compose_args(&integration, &["restart"]);
        assert!(
            !up.iter()
                .chain(&stop)
                .chain(&restart)
                .any(|arg| arg == "down")
        );
    }

    #[test]
    fn connection_save_preserves_dpapi_envelope_and_never_stores_plain_token() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("newapi-integration.json");
        save_api_key_to(&path, "sk-secret-value").unwrap();
        save_connection_settings_to(
            &path,
            NewApiSaveConnectionRequest {
                project_root: temp.path().to_string_lossy().to_string(),
                compose_file: "compose.yaml".to_string(),
                docker_executable: "docker".to_string(),
                api_service_name: "new-api".to_string(),
                base_url: "http://localhost:3000/v1".to_string(),
            },
        )
        .unwrap();
        let stored = fs::read_to_string(&path).unwrap();
        assert!(!stored.contains("sk-secret-value"));
        let loaded = load_integration_settings_from(&path).unwrap();
        assert_eq!(loaded.api_key().unwrap(), "sk-secret-value");
        assert_eq!(loaded.base_url, "http://localhost:3000");
    }

    #[test]
    fn apply_profile_creates_then_updates_one_ordinary_responses_profile() {
        let temp = tempfile::tempdir().unwrap();
        let integration = temp_integration(temp.path());
        let store = SettingsStore::new(temp.path().join("settings.json"));
        let mut settings = BackendSettings::default();
        settings.relay_profiles = vec![RelayProfile {
            id: "keep".to_string(),
            name: "Keep".to_string(),
            ..RelayProfile::default()
        }];
        store.save(&settings).unwrap();

        let created = apply_profile_with(
            NewApiApplyRequest {
                model: "vendor/model-b".to_string(),
                models: vec![
                    "model-a".to_string(),
                    "vendor/model-b".to_string(),
                    "model-a".to_string(),
                ],
            },
            &integration,
            "sk-test",
            &store,
        )
        .unwrap();
        assert!(created.created);
        let profile = created
            .settings
            .relay_profiles
            .iter()
            .find(|profile| profile.id == MANAGED_PROFILE_ID)
            .unwrap();
        assert_eq!(profile.integration_type, INTEGRATION_TYPE);
        assert_eq!(profile.relay_mode, RelayMode::PureApi);
        assert_eq!(profile.protocol, RelayProtocol::Responses);
        assert_eq!(profile.base_url, "http://127.0.0.1:3000/v1");
        assert_eq!(profile.model, "vendor/model-b");
        assert!(profile.model_list.contains("model-a"));

        let updated = apply_profile_with(
            NewApiApplyRequest {
                model: "model-c".to_string(),
                models: vec!["model-c".to_string()],
            },
            &integration,
            "sk-new",
            &store,
        )
        .unwrap();
        assert!(!updated.created);
        assert_eq!(
            updated
                .settings
                .relay_profiles
                .iter()
                .filter(|profile| is_managed_profile(profile))
                .count(),
            1
        );
        let profile = updated
            .settings
            .relay_profiles
            .iter()
            .find(|profile| is_managed_profile(profile))
            .unwrap();
        assert_eq!(profile.model, "model-c");
        assert!(profile.config_contents.contains("sk-new"));
    }

    #[test]
    fn disable_removes_profile_aggregate_references_and_repairs_active_relay() {
        let temp = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(temp.path().join("settings.json"));
        let mut settings = BackendSettings::default();
        settings.active_relay_id = MANAGED_PROFILE_ID.to_string();
        settings.active_aggregate_relay_id = "only-newapi".to_string();
        settings.relay_profiles = vec![
            RelayProfile {
                id: "fallback".to_string(),
                name: "Fallback".to_string(),
                ..RelayProfile::default()
            },
            RelayProfile {
                id: MANAGED_PROFILE_ID.to_string(),
                integration_type: INTEGRATION_TYPE.to_string(),
                ..RelayProfile::default()
            },
            RelayProfile {
                id: "only-newapi".to_string(),
                relay_mode: RelayMode::Aggregate,
                ..RelayProfile::default()
            },
        ];
        settings.aggregate_relay_profiles = vec![AggregateRelayProfile {
            id: "only-newapi".to_string(),
            name: "Only NewAPI".to_string(),
            session_provider: RelaySessionProvider::Custom,
            strategy: AggregateRelayStrategy::Failover,
            model_mappings_enabled: true,
            members: vec![AggregateRelayMember {
                relay_id: MANAGED_PROFILE_ID.to_string(),
                weight: 1,
            }],
            model_mappings: vec![AggregateRelayModelMapping {
                codex_model: "gpt-test".to_string(),
                targets: vec![AggregateRelayDispatchTarget {
                    relay_id: MANAGED_PROFILE_ID.to_string(),
                    target_model: "upstream-test".to_string(),
                }],
            }],
        }];
        store.save(&settings).unwrap();

        let disabled = disable_integration_with(&store).unwrap();
        assert!(
            disabled
                .settings
                .relay_profiles
                .iter()
                .all(|profile| !is_managed_profile(profile))
        );
        let aggregate = &disabled.settings.aggregate_relay_profiles[0];
        assert!(aggregate.members.is_empty());
        assert!(aggregate.model_mappings.is_empty());
        assert!(disabled.settings.active_aggregate_relay_id.is_empty());
        assert_eq!(disabled.settings.active_relay_id, "fallback");
    }
}
