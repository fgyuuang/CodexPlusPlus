use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, ensure};
use base64::Engine as _;
use codex_plus_core::settings::{
    BackendSettings, RelayMode, RelayModelInsertMode, RelayProtocol, SettingsStore,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zip::ZipArchive;

use crate::commands::CommandResult;

const RELEASE_VERSION: &str = "v7.2.103";
const RELEASE_FILE: &str = "CLIProxyAPI_7.2.103_windows_amd64.zip";
const RELEASE_BASE_URL: &str =
    "https://github.com/router-for-me/CLIProxyAPI/releases/download/v7.2.103";
const INSTALL_ROOT: &str = r"D:\pro\CLIProxyAPI";
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8317;
const GENERAL_PROFILE_ID: &str = "managed-cliproxy";
const OFFICIAL_PROFILE_ID: &str = "managed-cliproxy-official";
const GENERAL_INTEGRATION_TYPE: &str = "cliproxy";
const OFFICIAL_INTEGRATION_TYPE: &str = "cliproxy-official";
const CHANNEL_GENERAL: &str = "generalRelay";
const CHANNEL_OFFICIAL: &str = "officialCodex";
const SECRET_BACKEND_DPAPI: &str = "windows-dpapi-current-user";
#[cfg(not(windows))]
const SECRET_BACKEND_USER_FILE: &str = "user-file";
const DPAPI_ENTROPY: &[u8] = b"CodexPlusPlus.CLIProxyAPI.v1";
const MAX_RELEASE_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone)]
struct Layout {
    root: PathBuf,
    release_dir: PathBuf,
    config_path: PathBuf,
    binary_override: Option<PathBuf>,
    service_url_override: Option<String>,
    runtime_dir: PathBuf,
    runtime_state_path: PathBuf,
    secrets_path: PathBuf,
    log_path: PathBuf,
}

impl Layout {
    fn new(root: PathBuf) -> Self {
        let release_dir = root.join("releases").join(RELEASE_VERSION);
        let config_path = root.join("config").join("config.yaml");
        let runtime_dir = root.join("runtime");
        Self {
            runtime_state_path: runtime_dir.join("service-state.json"),
            secrets_path: runtime_dir.join("secrets.json"),
            log_path: root.join("logs").join("cliproxy.log"),
            root,
            release_dir,
            config_path,
            binary_override: None,
            service_url_override: None,
            runtime_dir,
        }
    }

    fn default() -> Self {
        Self::new(PathBuf::from(INSTALL_ROOT))
    }

    fn configured() -> anyhow::Result<Self> {
        Ok(Self::from_integration_settings(
            &load_integration_settings()?
        ))
    }

    fn from_integration_settings(settings: &CliproxyIntegrationSettings) -> Self {
        let root = path_or_default(&settings.install_root, PathBuf::from(INSTALL_ROOT));
        let mut layout = Self::new(root.clone());
        layout.config_path = path_or_default(
            &settings.config_path,
            root.join("config").join("config.yaml"),
        );
        layout.binary_override = non_empty_path(&settings.binary_path);
        layout.service_url_override = normalize_service_url(&settings.base_url).ok();
        layout
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliproxyIntegrationSettings {
    #[serde(default = "default_install_root")]
    install_root: String,
    #[serde(default)]
    binary_path: String,
    #[serde(default)]
    config_path: String,
    #[serde(default)]
    base_url: String,
}

impl Default for CliproxyIntegrationSettings {
    fn default() -> Self {
        Self {
            install_root: default_install_root(),
            binary_path: String::new(),
            config_path: String::new(),
            base_url: String::new(),
        }
    }
}

fn default_install_root() -> String {
    INSTALL_ROOT.to_string()
}

fn integration_settings_path() -> PathBuf {
    codex_plus_core::paths::default_settings_path().with_file_name("cliproxy-integration.json")
}

fn load_integration_settings() -> anyhow::Result<CliproxyIntegrationSettings> {
    let path = integration_settings_path();
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("CLIProxyAPI 连接设置格式无效：{}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(CliproxyIntegrationSettings::default())
        }
        Err(error) => Err(error.into()),
    }
}

fn save_connection_settings(request: CliproxySaveConnectionRequest) -> anyhow::Result<()> {
    let install_root = absolute_path_text(&request.install_root, "安装目录")?;
    let binary_path = optional_absolute_path_text(&request.binary_path, "可执行文件")?;
    let config_path = optional_absolute_path_text(&request.config_path, "配置文件")?;
    let service_url = normalize_service_url(&request.base_url)?;
    let next = CliproxyIntegrationSettings {
        install_root,
        binary_path,
        config_path,
        base_url: openai_api_base_url(&service_url),
    };

    let current_layout = Layout::configured()?;
    if let Some(state) = load_runtime_state(&current_layout)?
        && process_matches(&state)
    {
        let next_layout = Layout::from_integration_settings(&next);
        ensure!(
            paths_equal(&current_layout.root, &next_layout.root)
                && paths_equal(&current_layout.config_path, &next_layout.config_path)
                && optional_paths_equal(
                    current_layout.binary_override.as_deref(),
                    next_layout.binary_override.as_deref()
                )
                && current_layout.service_url_override == next_layout.service_url_override,
            "CLIProxyAPI 由 Manager 启动且仍在运行，请先停止服务再修改启动位置"
        );
    }

    atomic_write_json(&integration_settings_path(), &next)
}

fn absolute_path_text(value: &str, label: &str) -> anyhow::Result<String> {
    let value = value.trim();
    ensure!(!value.is_empty(), "{label}不能为空");
    let path = PathBuf::from(value);
    ensure!(path.is_absolute(), "{label}必须是绝对路径");
    Ok(path.to_string_lossy().to_string())
}

fn optional_absolute_path_text(value: &str, label: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    absolute_path_text(value, label)
}

fn non_empty_path(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn path_or_default(value: &str, default: PathBuf) -> PathBuf {
    non_empty_path(value).unwrap_or(default)
}

fn optional_paths_equal(left: Option<&Path>, right: Option<&Path>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => paths_equal(left, right),
        (None, None) => true,
        _ => false,
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeState {
    version: u32,
    pid: u32,
    started_at: i64,
    binary_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecretsEnvelope {
    version: u32,
    backend: String,
    payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliproxySecrets {
    version: u32,
    api_key: String,
    management_key: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliproxyStatusPayload {
    pub installed: bool,
    pub running: bool,
    pub healthy: bool,
    pub managed_process: bool,
    pub pid: Option<u32>,
    pub started_at: Option<i64>,
    pub version: String,
    pub install_root: String,
    pub binary_path: String,
    pub config_path: String,
    pub base_url: String,
    pub management_url: String,
    pub api_key: String,
    pub management_key: String,
    pub profile_installed: bool,
    pub official_profile_installed: bool,
    pub general_profile_installed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliproxyModelsPayload {
    pub models: Vec<String>,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliproxyTestPayload {
    pub http_status: u16,
    pub endpoint: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliproxyApplyPayload {
    pub settings: BackendSettings,
    pub profile_id: String,
    pub created: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliproxyTestRequest {
    #[serde(default)]
    pub model: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliproxySaveApiKeyRequest {
    pub api_key: String,
    #[serde(default)]
    pub management_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliproxySaveConnectionRequest {
    pub install_root: String,
    #[serde(default)]
    pub binary_path: String,
    #[serde(default)]
    pub config_path: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliproxyApplyRequest {
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub models: Vec<String>,
}

#[tauri::command]
pub async fn cliproxy_status() -> CommandResult<CliproxyStatusPayload> {
    match status_payload().await {
        Ok(payload) => success("CLIProxyAPI 状态已刷新。", payload),
        Err(error) => failure(
            &format!("读取 CLIProxyAPI 状态失败：{error}"),
            fallback_status(),
        ),
    }
}

#[tauri::command]
pub async fn cliproxy_install() -> CommandResult<CliproxyStatusPayload> {
    match install_release().await {
        Ok(()) => match status_payload().await {
            Ok(payload) => success("CLIProxyAPI 已安装。", payload),
            Err(error) => failure(
                &format!("CLIProxyAPI 已安装，但读取状态失败：{error}"),
                fallback_status(),
            ),
        },
        Err(error) => failure(
            &format!("安装 CLIProxyAPI 失败：{error}"),
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
    }
}

#[tauri::command]
pub async fn cliproxy_start() -> CommandResult<CliproxyStatusPayload> {
    match start_service().await {
        Ok(()) => success(
            "CLIProxyAPI 已启动。",
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
        Err(error) => failure(
            &format!("启动 CLIProxyAPI 失败：{error}"),
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
    }
}

#[tauri::command]
pub async fn cliproxy_stop() -> CommandResult<CliproxyStatusPayload> {
    match stop_service().await {
        Ok(()) => success(
            "CLIProxyAPI 已停止。",
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
        Err(error) => failure(
            &format!("停止 CLIProxyAPI 失败：{error}"),
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
    }
}

#[tauri::command]
pub async fn cliproxy_restart() -> CommandResult<CliproxyStatusPayload> {
    if let Err(error) = stop_service().await {
        return failure(
            &format!("重启 CLIProxyAPI 失败：{error}"),
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        );
    }
    match start_service().await {
        Ok(()) => success(
            "CLIProxyAPI 已重启。",
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
        Err(error) => failure(
            &format!("重启 CLIProxyAPI 失败：{error}"),
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
    }
}

#[tauri::command]
pub async fn cliproxy_open_management() -> CommandResult<Value> {
    let url = status_payload()
        .await
        .map(|status| status.management_url)
        .unwrap_or_else(|_| fallback_status().management_url);
    crate::commands::open_external_url(url)
}

#[tauri::command]
pub async fn cliproxy_list_models() -> CommandResult<CliproxyModelsPayload> {
    match list_models_payload().await {
        Ok(payload) => success(
            &format!("CLIProxyAPI 返回了 {} 个模型。", payload.models.len()),
            payload,
        ),
        Err(error) => failure(
            &format!("读取 CLIProxyAPI 模型失败：{error}"),
            CliproxyModelsPayload {
                models: Vec::new(),
                endpoint: String::new(),
            },
        ),
    }
}

#[tauri::command]
pub async fn cliproxy_test_api(request: CliproxyTestRequest) -> CommandResult<CliproxyTestPayload> {
    match test_api(&request.model).await {
        Ok(payload) => success(
            &format!(
                "CLIProxyAPI 请求成功，模型「{}」，HTTP {}。",
                payload.model, payload.http_status
            ),
            payload,
        ),
        Err(error) => failure(
            &format!("测试 CLIProxyAPI 失败：{error}"),
            CliproxyTestPayload {
                http_status: 0,
                endpoint: String::new(),
                model: request.model,
            },
        ),
    }
}

#[tauri::command]
pub async fn cliproxy_save_api_key(
    request: CliproxySaveApiKeyRequest,
) -> CommandResult<CliproxyStatusPayload> {
    let api_key = request.api_key.trim();
    let management_key = request.management_key.trim();
    if api_key.is_empty() && management_key.is_empty() {
        return failure("CLIProxyAPI 连接密钥不能为空。", fallback_status());
    }
    let layout = match Layout::configured() {
        Ok(layout) => layout,
        Err(error) => {
            return failure(
                &format!("读取 CLIProxyAPI 连接设置失败：{error}"),
                fallback_status(),
            );
        }
    };
    match ensure_secrets(&layout).and_then(|mut secrets| {
        if !api_key.is_empty() {
            secrets.api_key = api_key.to_string();
        }
        if !management_key.is_empty() {
            secrets.management_key = management_key.to_string();
        }
        save_secrets(&layout, &secrets)
    }) {
        Ok(()) => success(
            "CLIProxyAPI 连接密钥已保存；不会修改 CLIProxyAPI 配置文件，重启后管理密钥生效。",
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
        Err(error) => failure(
            &format!("保存 CLIProxyAPI API Key 失败：{error}"),
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
    }
}

#[tauri::command]
pub async fn cliproxy_save_connection(
    request: CliproxySaveConnectionRequest,
) -> CommandResult<CliproxyStatusPayload> {
    match save_connection_settings(request) {
        Ok(()) => match status_payload().await {
            Ok(payload) => success("CLIProxyAPI 启动与连接位置已保存。", payload),
            Err(error) => failure(
                &format!("CLIProxyAPI 连接设置已保存，但刷新状态失败：{error}"),
                fallback_status(),
            ),
        },
        Err(error) => failure(
            &format!("保存 CLIProxyAPI 启动与连接位置失败：{error}"),
            status_payload().await.unwrap_or_else(|_| fallback_status()),
        ),
    }
}

#[tauri::command]
pub async fn cliproxy_apply_profile(
    request: CliproxyApplyRequest,
) -> CommandResult<CliproxyApplyPayload> {
    let official_channel = request.channel.trim() == CHANNEL_OFFICIAL;
    match apply_profile(request) {
        Ok(payload) => success(
            if official_channel {
                "CLIProxyAPI 官方模型已启用；它不会出现在供应商列表或聚合成员中。"
            } else {
                "CLIProxyAPI 接入已启用；所有 CLIProxyAPI 模型均作为受管直连供应商保存。"
            },
            payload,
        ),
        Err(error) => failure(
            &format!("保存 CLIProxyAPI 供应商失败：{error}"),
            CliproxyApplyPayload {
                settings: SettingsStore::default().load().unwrap_or_default(),
                profile_id: GENERAL_PROFILE_ID.to_string(),
                created: false,
            },
        ),
    }
}

#[tauri::command]
pub async fn cliproxy_disable_official_profile() -> CommandResult<CliproxyApplyPayload> {
    match remove_official_profile() {
        Ok(payload) => success(
            "CLIProxyAPI 官方模型已关闭；通用中转和 CLIProxyAPI 服务未受影响。",
            payload,
        ),
        Err(error) => failure(
            &format!("关闭 CLIProxyAPI 官方模型失败：{error}"),
            CliproxyApplyPayload {
                settings: SettingsStore::default().load().unwrap_or_default(),
                profile_id: OFFICIAL_PROFILE_ID.to_string(),
                created: false,
            },
        ),
    }
}

#[tauri::command]
pub async fn cliproxy_disable_integration() -> CommandResult<CliproxyApplyPayload> {
    match remove_integration_profiles() {
        Ok(payload) => success(
            "CLIProxyAPI 接入已关闭；CLIProxyAPI 服务和账号文件未受影响。",
            payload,
        ),
        Err(error) => failure(
            &format!("关闭 CLIProxyAPI 接入失败：{error}"),
            CliproxyApplyPayload {
                settings: SettingsStore::default().load().unwrap_or_default(),
                profile_id: GENERAL_PROFILE_ID.to_string(),
                created: false,
            },
        ),
    }
}

async fn install_release() -> anyhow::Result<()> {
    ensure!(cfg!(windows), "CLIProxyAPI 自动安装当前仅支持 Windows");
    let layout = Layout::configured()?;
    prepare_directories(&layout)?;
    if locate_binary_for_layout(&layout).is_some() {
        ensure_config(&layout)?;
        return Ok(());
    }

    let checksums_url = format!("{RELEASE_BASE_URL}/checksums.txt");
    let archive_url = format!("{RELEASE_BASE_URL}/{RELEASE_FILE}");
    let checksums = download_bytes(&checksums_url, 2 * 1024 * 1024).await?;
    let expected = checksum_for_asset(&checksums, RELEASE_FILE)?;
    let archive = download_bytes(&archive_url, MAX_RELEASE_BYTES).await?;
    let actual = hex_digest(&archive);
    ensure!(
        actual.eq_ignore_ascii_case(&expected),
        "下载包 SHA-256 校验失败"
    );

    let releases_dir = layout.root.join("releases");
    let staging = releases_dir.join(format!(".staging-{}", Uuid::new_v4()));
    fs::create_dir_all(&staging)?;
    let extraction_result = extract_archive(&archive, &staging)
        .and_then(|_| locate_binary(&staging).context("安装包中未找到 CLIProxyAPI 可执行文件"));
    let staged_binary = match extraction_result {
        Ok(path) => path,
        Err(error) => {
            remove_staging_dir(&releases_dir, &staging);
            return Err(error);
        }
    };
    let target_binary = staging.join("cli-proxy-api.exe");
    if staged_binary != target_binary {
        fs::copy(&staged_binary, &target_binary)?;
    }
    let release_marker = json!({
        "version": RELEASE_VERSION,
        "asset": RELEASE_FILE,
        "sha256": actual,
    });
    atomic_write_json(&staging.join("release.json"), &release_marker)?;
    if layout.release_dir.exists() {
        remove_staging_dir(&releases_dir, &staging);
        anyhow::bail!("目标版本目录已存在但缺少可执行文件，请人工检查");
    }
    fs::rename(&staging, &layout.release_dir)?;
    ensure_config(&layout)?;
    Ok(())
}

async fn start_service() -> anyhow::Result<()> {
    let layout = Layout::configured()?;
    prepare_directories(&layout)?;
    let binary = locate_binary_for_layout(&layout).context("CLIProxyAPI 尚未安装")?;
    ensure_config(&layout)?;
    let connection = connection_info(&layout)?;
    if health_check(&connection.base_url).await.is_ok() {
        return Ok(());
    }
    let secrets = ensure_secrets(&layout)?;
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&layout.log_path)?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&layout.log_path)?;
    let mut command = Command::new(&binary);
    let working_directory = binary.parent().unwrap_or(&layout.release_dir);
    command
        .arg("-config")
        .arg(&layout.config_path)
        .current_dir(working_directory)
        .env("MANAGEMENT_PASSWORD", &secrets.management_key)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000 | 0x0000_0200);
    }
    let child = command.spawn().context("创建 CLIProxyAPI 进程失败")?;
    let state = RuntimeState {
        version: 1,
        pid: child.id(),
        started_at: now_ts(),
        binary_path: binary.to_string_lossy().to_string(),
    };
    atomic_write_json(&layout.runtime_state_path, &state)?;
    drop(child);

    for _ in 0..30 {
        if health_check(&connection.base_url).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    if process_matches(&state) {
        let _ = terminate_process(state.pid);
    }
    clear_runtime_state(&layout);
    anyhow::bail!("服务未在 12 秒内通过 /healthz 检查")
}

async fn stop_service() -> anyhow::Result<()> {
    let layout = Layout::configured()?;
    let Some(state) = load_runtime_state(&layout)? else {
        let connection = connection_info(&layout).unwrap_or_else(|_| ConnectionInfo::default());
        if health_check(&connection.base_url).await.is_ok() {
            anyhow::bail!("检测到外部启动的 CLIProxyAPI；为避免误杀，Manager 不会停止它");
        }
        return Ok(());
    };
    if !process_matches(&state) {
        clear_runtime_state(&layout);
        let connection = connection_info(&layout).unwrap_or_else(|_| ConnectionInfo::default());
        if health_check(&connection.base_url).await.is_ok() {
            anyhow::bail!("PID 记录已失效且端口上存在其他服务，已拒绝停止");
        }
        return Ok(());
    }
    ensure!(
        terminate_process(state.pid),
        "无法停止已核验的 CLIProxyAPI 进程"
    );
    for _ in 0..25 {
        if !process_matches(&state) {
            clear_runtime_state(&layout);
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    anyhow::bail!("CLIProxyAPI 进程未在超时前退出")
}

async fn status_payload() -> anyhow::Result<CliproxyStatusPayload> {
    let layout = Layout::configured()?;
    let binary = locate_binary_for_layout(&layout);
    let connection = connection_info(&layout).unwrap_or_else(|_| ConnectionInfo::default());
    let health = health_check(&connection.base_url).await.ok();
    let state = load_runtime_state(&layout)?;
    let valid_state = state.as_ref().filter(|state| process_matches(state));
    if state.is_some() && valid_state.is_none() {
        clear_runtime_state(&layout);
    }
    let secrets = load_secrets(&layout).ok().flatten();
    let settings = SettingsStore::default().load().unwrap_or_default();
    let installed_version = if binary.is_some() {
        RELEASE_VERSION.to_string()
    } else {
        String::new()
    };
    Ok(CliproxyStatusPayload {
        installed: binary.is_some(),
        running: health.is_some(),
        healthy: health.is_some(),
        managed_process: valid_state.is_some(),
        pid: valid_state.map(|state| state.pid),
        started_at: valid_state.map(|state| state.started_at),
        version: health
            .and_then(|health| health.version)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(installed_version),
        install_root: layout.root.to_string_lossy().to_string(),
        binary_path: binary
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| preferred_binary_path(&layout).to_string_lossy().to_string()),
        config_path: layout.config_path.to_string_lossy().to_string(),
        base_url: openai_api_base_url(&connection.base_url),
        management_url: format!(
            "{}/management.html",
            connection.base_url.trim_end_matches('/')
        ),
        api_key: secrets
            .as_ref()
            .map(|value| value.api_key.clone())
            .unwrap_or_default(),
        management_key: secrets
            .map(|value| value.management_key)
            .unwrap_or_default(),
        profile_installed: settings.relay_profiles.iter().any(is_managed_profile),
        official_profile_installed: settings
            .relay_profiles
            .iter()
            .any(is_official_managed_profile),
        general_profile_installed: settings
            .relay_profiles
            .iter()
            .any(is_general_managed_profile),
    })
}

async fn list_models_payload() -> anyhow::Result<CliproxyModelsPayload> {
    let layout = Layout::configured()?;
    let connection = connection_info(&layout)?;
    let secrets = ensure_secrets(&layout)?;
    let endpoint = format!("{}/v1/models", connection.base_url.trim_end_matches('/'));
    let response = http_client(Duration::from_secs(30))?
        .get(&endpoint)
        .bearer_auth(&secrets.api_key)
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
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Ok(CliproxyModelsPayload { models, endpoint })
}

async fn test_api(requested_model: &str) -> anyhow::Result<CliproxyTestPayload> {
    let models = list_models_payload().await?;
    let model = if requested_model.trim().is_empty() {
        models
            .models
            .first()
            .cloned()
            .context("CLIProxyAPI 未返回可测试模型")?
    } else {
        requested_model.trim().to_string()
    };
    ensure!(
        models.models.iter().any(|item| item == &model),
        "测试模型不在 CLIProxyAPI 模型列表中"
    );
    let layout = Layout::configured()?;
    let connection = connection_info(&layout)?;
    let secrets = ensure_secrets(&layout)?;
    let endpoint = format!("{}/v1/responses", connection.base_url.trim_end_matches('/'));
    let response = http_client(Duration::from_secs(90))?
        .post(&endpoint)
        .bearer_auth(&secrets.api_key)
        .json(&json!({
            "model": model,
            "input": "hi",
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
    Ok(CliproxyTestPayload {
        http_status: status.as_u16(),
        endpoint,
        model,
    })
}

fn apply_profile(request: CliproxyApplyRequest) -> anyhow::Result<CliproxyApplyPayload> {
    let layout = Layout::configured()?;
    let store = SettingsStore::default();
    apply_profile_with(request, &layout, &store)
}

fn remove_official_profile() -> anyhow::Result<CliproxyApplyPayload> {
    let store = SettingsStore::default();
    remove_official_profile_with(&store)
}

fn remove_official_profile_with(store: &SettingsStore) -> anyhow::Result<CliproxyApplyPayload> {
    remove_profiles_with(store, false)
}

fn remove_integration_profiles() -> anyhow::Result<CliproxyApplyPayload> {
    let store = SettingsStore::default();
    remove_integration_profiles_with(&store)
}

fn remove_integration_profiles_with(store: &SettingsStore) -> anyhow::Result<CliproxyApplyPayload> {
    remove_profiles_with(store, true)
}

fn remove_profiles_with(
    store: &SettingsStore,
    remove_integration: bool,
) -> anyhow::Result<CliproxyApplyPayload> {
    let mut settings = store.load().unwrap_or_default();
    let removed_active_profile = settings
        .relay_profiles
        .iter()
        .find(|profile| profile.id == settings.active_relay_id)
        .is_some_and(|profile| {
            if remove_integration {
                is_managed_profile(profile)
            } else {
                is_official_managed_profile(profile)
            }
        });
    settings.relay_profiles.retain(|profile| {
        if remove_integration {
            !is_managed_profile(profile)
        } else {
            !is_official_managed_profile(profile)
        }
    });
    if removed_active_profile {
        settings.active_relay_id = settings
            .relay_profiles
            .iter()
            .find(|profile| profile.id == settings.active_aggregate_relay_id)
            .or_else(|| settings.relay_profiles.first())
            .map(|profile| profile.id.clone())
            .unwrap_or_else(|| "default".to_string());
    }
    store.save(&settings)?;
    Ok(CliproxyApplyPayload {
        settings,
        profile_id: if remove_integration {
            GENERAL_PROFILE_ID.to_string()
        } else {
            OFFICIAL_PROFILE_ID.to_string()
        },
        created: false,
    })
}

fn apply_profile_with(
    request: CliproxyApplyRequest,
    layout: &Layout,
    store: &SettingsStore,
) -> anyhow::Result<CliproxyApplyPayload> {
    let channel = requested_channel(&request)?;
    let connection = connection_info(layout)?;
    let secrets = ensure_secrets(layout)?;
    ensure!(
        !secrets.api_key.trim().is_empty(),
        "CLIProxyAPI API Key 不能为空"
    );
    let mut settings = store.load().unwrap_or_default();
    let (profile_id, integration_type, profile_name) = match channel {
        CHANNEL_OFFICIAL => (
            OFFICIAL_PROFILE_ID,
            OFFICIAL_INTEGRATION_TYPE,
            "CLIProxyAPI 官方 Codex API",
        ),
        CHANNEL_GENERAL => (GENERAL_PROFILE_ID, GENERAL_INTEGRATION_TYPE, "CLIProxyAPI"),
        _ => unreachable!(),
    };
    let existing_index = settings.relay_profiles.iter().position(|profile| {
        profile.id == profile_id || profile.integration_type == integration_type
    });
    let created = existing_index.is_none();
    let mut models = request
        .models
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| model_belongs_to_channel(value, channel))
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    ensure!(
        !models.is_empty(),
        if channel == CHANNEL_OFFICIAL {
            "CLIProxyAPI 未返回可信官方 Codex 模型"
        } else {
            "CLIProxyAPI 未返回可用于通用中转的其他模型"
        }
    );
    let requested_model = request.model.trim();
    let model = models
        .iter()
        .find(|item| item.as_str() == requested_model)
        .cloned()
        .unwrap_or_else(|| models[0].clone());
    let mut profile = existing_index
        .and_then(|index| settings.relay_profiles.get(index).cloned())
        .unwrap_or_default();
    profile.id = profile_id.to_string();
    profile.name = profile_name.to_string();
    profile.integration_type = integration_type.to_string();
    profile.model = model.clone();
    let api_base_url = openai_api_base_url(&connection.base_url);
    profile.base_url = api_base_url.clone();
    profile.upstream_base_url = api_base_url;
    profile.api_key = secrets.api_key.clone();
    profile.protocol = RelayProtocol::Responses;
    profile.relay_mode = RelayMode::PureApi;
    profile.official_mix_api_key = false;
    profile.test_model = model;
    profile.config_contents.clear();
    profile.auth_contents = serde_json::to_string_pretty(&json!({
        "OPENAI_API_KEY": secrets.api_key,
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
    store.save(&settings)?;
    Ok(CliproxyApplyPayload {
        settings,
        profile_id: profile_id.to_string(),
        created,
    })
}

fn requested_channel(request: &CliproxyApplyRequest) -> anyhow::Result<&'static str> {
    match request.channel.trim() {
        CHANNEL_OFFICIAL => Ok(CHANNEL_OFFICIAL),
        CHANNEL_GENERAL => Ok(CHANNEL_GENERAL),
        "" if request.mode.trim() == "mixedApi" || request.mode.trim() == "pureApi" => {
            Ok(CHANNEL_GENERAL)
        }
        _ => anyhow::bail!("不支持的 CLIProxyAPI 接入通道"),
    }
}

fn model_belongs_to_channel(model: &str, channel: &str) -> bool {
    if channel == CHANNEL_OFFICIAL {
        cliproxy_model_is_official(model)
    } else {
        true
    }
}

fn cliproxy_model_is_official(model: &str) -> bool {
    let model = model.trim();
    let base_model = model.rsplit('/').next().unwrap_or(model).trim();
    codex_plus_core::aggregate_model_alias::is_trusted_official_codex_model(base_model)
}

fn is_managed_profile(profile: &codex_plus_core::settings::RelayProfile) -> bool {
    is_official_managed_profile(profile) || is_general_managed_profile(profile)
}

fn is_official_managed_profile(profile: &codex_plus_core::settings::RelayProfile) -> bool {
    profile.id == OFFICIAL_PROFILE_ID || profile.integration_type == OFFICIAL_INTEGRATION_TYPE
}

fn is_general_managed_profile(profile: &codex_plus_core::settings::RelayProfile) -> bool {
    profile.id == GENERAL_PROFILE_ID || profile.integration_type == GENERAL_INTEGRATION_TYPE
}

#[derive(Debug, Clone)]
struct ConnectionInfo {
    base_url: String,
}

impl Default for ConnectionInfo {
    fn default() -> Self {
        Self {
            base_url: format!("http://{DEFAULT_HOST}:{DEFAULT_PORT}"),
        }
    }
}

fn connection_info(layout: &Layout) -> anyhow::Result<ConnectionInfo> {
    if let Some(base_url) = layout.service_url_override.as_deref() {
        return Ok(ConnectionInfo {
            base_url: base_url.to_string(),
        });
    }
    if !layout.config_path.exists() {
        return Ok(ConnectionInfo::default());
    }
    let value: serde_yaml::Value = serde_yaml::from_slice(&fs::read(&layout.config_path)?)?;
    let host = value
        .get("host")
        .and_then(serde_yaml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_HOST);
    let connect_host = match host {
        "0.0.0.0" | "::" => DEFAULT_HOST,
        other => other,
    };
    let port = value
        .get("port")
        .and_then(serde_yaml::Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(DEFAULT_PORT);
    Ok(ConnectionInfo {
        base_url: format!("http://{connect_host}:{port}"),
    })
}

fn openai_api_base_url(service_url: &str) -> String {
    let service_url = service_url.trim_end_matches('/');
    if service_url.ends_with("/v1") {
        service_url.to_string()
    } else {
        format!("{service_url}/v1")
    }
}

#[derive(Debug)]
struct HealthInfo {
    version: Option<String>,
}

async fn health_check(base_url: &str) -> anyhow::Result<HealthInfo> {
    let endpoint = format!("{}/healthz", base_url.trim_end_matches('/'));
    let response = http_client(Duration::from_secs(3))?
        .get(endpoint)
        .send()
        .await?;
    ensure!(response.status().is_success(), "健康检查未通过");
    let version = response
        .headers()
        .get("x-cpa-version")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    Ok(HealthInfo { version })
}

fn http_client(timeout: Duration) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(timeout)
        .user_agent(format!("codex-plus-plus/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("创建 CLIProxyAPI HTTP 客户端失败")
}

fn prepare_directories(layout: &Layout) -> anyhow::Result<()> {
    fs::create_dir_all(layout.root.join("releases"))?;
    fs::create_dir_all(layout.root.join("config"))?;
    fs::create_dir_all(&layout.runtime_dir)?;
    fs::create_dir_all(layout.root.join("logs"))?;
    Ok(())
}

fn ensure_config(layout: &Layout) -> anyhow::Result<()> {
    prepare_directories(layout)?;
    let secrets = ensure_secrets(layout)?;
    if layout.config_path.exists() {
        return Ok(());
    }
    let config = serde_yaml::to_string(&json!({
        "host": DEFAULT_HOST,
        "port": DEFAULT_PORT,
        "remote-management": {
            "allow-remote": false,
            "secret-key": "",
            "disable-control-panel": false,
        },
        "auth-dir": "~/.cli-proxy-api",
        "api-keys": [secrets.api_key],
        "debug": false,
        "force-model-prefix": false,
        "routing": {
            "strategy": "round-robin",
            "session-affinity": false,
        },
    }))?;
    codex_plus_core::settings::atomic_write(&layout.config_path, config.as_bytes())
}

fn ensure_secrets(layout: &Layout) -> anyhow::Result<CliproxySecrets> {
    if let Some(secrets) = load_secrets(layout)? {
        return Ok(secrets);
    }
    fs::create_dir_all(&layout.runtime_dir)?;
    let api_key = existing_config_api_key(&layout.config_path)
        .unwrap_or_else(|| format!("cpp-{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()));
    let secrets = CliproxySecrets {
        version: 1,
        api_key,
        management_key: format!(
            "mgmt-{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        ),
    };
    save_secrets(layout, &secrets)?;
    Ok(secrets)
}

fn existing_config_api_key(path: &Path) -> Option<String> {
    let value: serde_yaml::Value = serde_yaml::from_slice(&fs::read(path).ok()?).ok()?;
    value
        .get("api-keys")
        .and_then(serde_yaml::Value::as_sequence)
        .and_then(|keys| keys.first())
        .and_then(serde_yaml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn save_secrets(layout: &Layout, secrets: &CliproxySecrets) -> anyhow::Result<()> {
    let plaintext = serde_json::to_vec(secrets)?;
    let (backend, protected) = protect_local_secret(&plaintext)?;
    let envelope = SecretsEnvelope {
        version: 1,
        backend,
        payload: base64::engine::general_purpose::STANDARD.encode(protected),
    };
    atomic_write_json(&layout.secrets_path, &envelope)
}

fn load_secrets(layout: &Layout) -> anyhow::Result<Option<CliproxySecrets>> {
    let bytes = match fs::read(&layout.secrets_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let envelope: SecretsEnvelope = serde_json::from_slice(&bytes)?;
    let protected = base64::engine::general_purpose::STANDARD
        .decode(envelope.payload.as_bytes())
        .context("CLIProxyAPI 凭据封装无效")?;
    let plaintext = unprotect_local_secret(&envelope.backend, &protected)?;
    Ok(Some(serde_json::from_slice(&plaintext)?))
}

fn load_runtime_state(layout: &Layout) -> anyhow::Result<Option<RuntimeState>> {
    match fs::read(&layout.runtime_state_path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn clear_runtime_state(layout: &Layout) {
    let _ = fs::remove_file(&layout.runtime_state_path);
}

fn process_matches(state: &RuntimeState) -> bool {
    let expected = PathBuf::from(&state.binary_path);
    process_image_path(state.pid).is_some_and(|path| paths_equal(&path, &expected))
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(windows)]
fn process_image_path(process_id: u32) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::core::PWSTR;

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()? };
    if handle.is_invalid() {
        return None;
    }
    let mut buffer = vec![0u16; 32_768];
    let mut length = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            handle,
            Default::default(),
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    result.ok()?;
    Some(PathBuf::from(OsString::from_wide(
        &buffer[..length as usize],
    )))
}

#[cfg(not(windows))]
fn process_image_path(_process_id: u32) -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn terminate_process(process_id: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
    };

    let Ok(handle) = (unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
            false,
            process_id,
        )
    }) else {
        return false;
    };
    if handle.is_invalid() {
        return false;
    }
    let result = unsafe { TerminateProcess(handle, 0) }.is_ok();
    let _ = unsafe { CloseHandle(handle) };
    result
}

#[cfg(not(windows))]
fn terminate_process(_process_id: u32) -> bool {
    false
}

fn locate_binary(root: &Path) -> Option<PathBuf> {
    let direct = root.join("cli-proxy-api.exe");
    if direct.is_file() {
        return Some(direct);
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = locate_binary(&path) {
                return Some(found);
            }
        } else if path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("cli-proxy-api.exe")
                    || name.eq_ignore_ascii_case("CLIProxyAPI.exe")
            })
        {
            return Some(path);
        }
    }
    None
}

fn locate_binary_for_layout(layout: &Layout) -> Option<PathBuf> {
    layout
        .binary_override
        .as_ref()
        .filter(|path| path.is_file())
        .cloned()
        .or_else(|| locate_binary(&layout.release_dir))
}

fn preferred_binary_path(layout: &Layout) -> PathBuf {
    layout
        .binary_override
        .clone()
        .unwrap_or_else(|| layout.release_dir.join("cli-proxy-api.exe"))
}

async fn download_bytes(url: &str, max_bytes: u64) -> anyhow::Result<Vec<u8>> {
    let response = http_client(Duration::from_secs(180))?
        .get(url)
        .send()
        .await
        .with_context(|| format!("下载失败：{url}"))?;
    ensure!(
        response.status().is_success(),
        "下载返回 HTTP {}",
        response.status()
    );
    if let Some(length) = response.content_length() {
        ensure!(length <= max_bytes, "下载文件超过大小限制");
    }
    let bytes = response.bytes().await?;
    ensure!(bytes.len() as u64 <= max_bytes, "下载文件超过大小限制");
    Ok(bytes.to_vec())
}

fn checksum_for_asset(checksums: &[u8], asset: &str) -> anyhow::Result<String> {
    let text = std::str::from_utf8(checksums).context("checksums.txt 不是 UTF-8")?;
    text.lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next()?;
            let name = fields.next()?.trim_start_matches('*');
            name.eq_ignore_ascii_case(asset).then(|| hash.to_string())
        })
        .context("checksums.txt 中未找到 Windows amd64 安装包")
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn extract_archive(bytes: &[u8], destination: &Path) -> anyhow::Result<()> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let output = destination.join(relative);
        ensure!(output.starts_with(destination), "安装包包含越界路径");
        if entry.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create(&output)?;
        std::io::copy(&mut entry, &mut file)?;
        file.flush()?;
    }
    Ok(())
}

fn remove_staging_dir(releases_dir: &Path, staging: &Path) {
    let releases = releases_dir
        .canonicalize()
        .unwrap_or_else(|_| releases_dir.to_path_buf());
    let target = staging
        .canonicalize()
        .unwrap_or_else(|_| staging.to_path_buf());
    if target.starts_with(&releases)
        && target
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with(".staging-"))
    {
        let _ = fs::remove_dir_all(target);
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

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn fallback_status() -> CliproxyStatusPayload {
    let layout = Layout::configured().unwrap_or_else(|_| Layout::default());
    let connection = connection_info(&layout).unwrap_or_default();
    CliproxyStatusPayload {
        installed: false,
        running: false,
        healthy: false,
        managed_process: false,
        pid: None,
        started_at: None,
        version: String::new(),
        install_root: layout.root.to_string_lossy().to_string(),
        binary_path: preferred_binary_path(&layout).to_string_lossy().to_string(),
        config_path: layout.config_path.to_string_lossy().to_string(),
        base_url: openai_api_base_url(&connection.base_url),
        management_url: format!("{}/management.html", connection.base_url),
        api_key: String::new(),
        management_key: String::new(),
        profile_installed: false,
        official_profile_installed: false,
        general_profile_installed: false,
    }
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
        cbData: u32::try_from(bytes.len()).context("CLIProxyAPI 凭据过大")?,
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
        cbData: u32::try_from(bytes.len()).context("CLIProxyAPI 凭据过大")?,
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

    #[test]
    fn custom_integration_settings_control_manager_paths_and_url() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("custom-cli");
        let binary = temp.path().join("programs").join("cli-proxy-api.exe");
        let config = temp.path().join("settings").join("cliproxy.yaml");
        let settings = CliproxyIntegrationSettings {
            install_root: root.to_string_lossy().to_string(),
            binary_path: binary.to_string_lossy().to_string(),
            config_path: config.to_string_lossy().to_string(),
            base_url: "http://127.0.0.1:9123/v1".to_string(),
        };

        let layout = Layout::from_integration_settings(&settings);
        assert!(paths_equal(&layout.root, &root));
        assert!(paths_equal(
            layout.binary_override.as_deref().unwrap(),
            &binary
        ));
        assert!(paths_equal(&layout.config_path, &config));
        assert_eq!(
            layout.service_url_override.as_deref(),
            Some("http://127.0.0.1:9123")
        );

        let serialized = serde_json::to_string(&settings).unwrap();
        assert!(!serialized.contains("apiKey"));
        assert!(!serialized.contains("managementKey"));
    }

    #[test]
    fn connection_url_validation_accepts_api_base_and_rejects_unsafe_urls() {
        assert_eq!(
            normalize_service_url("http://127.0.0.1:8317/v1/").unwrap(),
            "http://127.0.0.1:8317"
        );
        assert!(normalize_service_url("ftp://127.0.0.1:8317").is_err());
        assert!(normalize_service_url("http://user:pass@127.0.0.1:8317").is_err());
        assert!(normalize_service_url("http://127.0.0.1:8317?key=value").is_err());
    }

    #[test]
    fn manager_paths_must_be_absolute() {
        assert!(absolute_path_text("relative/path", "安装目录").is_err());
        assert_eq!(optional_absolute_path_text("", "可执行文件").unwrap(), "");
    }

    #[test]
    fn generated_config_is_local_and_does_not_touch_auth_files() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::new(temp.path().join("CLIProxyAPI"));
        ensure_config(&layout).unwrap();
        let config = fs::read_to_string(&layout.config_path).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&config).unwrap();
        assert_eq!(
            parsed.get("host").and_then(serde_yaml::Value::as_str),
            Some(DEFAULT_HOST)
        );
        assert_eq!(
            parsed.get("port").and_then(serde_yaml::Value::as_u64),
            Some(DEFAULT_PORT.into())
        );
        assert_eq!(
            parsed
                .get("remote-management")
                .and_then(|value| value.get("disable-control-panel"))
                .and_then(serde_yaml::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            parsed
                .get("force-model-prefix")
                .and_then(serde_yaml::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            parsed
                .get("routing")
                .and_then(|value| value.get("session-affinity"))
                .and_then(serde_yaml::Value::as_bool),
            Some(false)
        );
        assert!(!layout.root.join("auth").exists());
    }

    #[test]
    fn config_is_not_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::new(temp.path().join("CLIProxyAPI"));
        prepare_directories(&layout).unwrap();
        fs::write(&layout.config_path, "host: custom.example\nport: 9000\n").unwrap();
        ensure_config(&layout).unwrap();
        assert_eq!(
            fs::read_to_string(&layout.config_path).unwrap(),
            "host: custom.example\nport: 9000\n"
        );
    }

    #[test]
    fn secrets_are_protected_at_rest() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::new(temp.path().join("CLIProxyAPI"));
        let secrets = ensure_secrets(&layout).unwrap();
        let stored = fs::read_to_string(&layout.secrets_path).unwrap();
        assert!(!stored.contains(&secrets.api_key));
        assert_eq!(load_secrets(&layout).unwrap().unwrap(), secrets);
    }

    #[test]
    fn checksum_parser_selects_exact_asset() {
        let text = format!("abc  other.zip\n012345  {RELEASE_FILE}\n");
        assert_eq!(
            checksum_for_asset(text.as_bytes(), RELEASE_FILE).unwrap(),
            "012345"
        );
    }

    #[test]
    fn official_codex_channel_is_separate_and_filters_non_official_models() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::new(temp.path().join("CLIProxyAPI"));
        prepare_directories(&layout).unwrap();
        fs::write(&layout.config_path, "host: 127.0.0.1\nport: 9123\n").unwrap();
        save_secrets(
            &layout,
            &CliproxySecrets {
                version: 1,
                api_key: "test-api-key".to_string(),
                management_key: "test-management-key".to_string(),
            },
        )
        .unwrap();

        let store = SettingsStore::new(temp.path().join("settings.json"));
        let mut settings = BackendSettings::default();
        settings.official_login_mixed_mode = true;
        let mut other = codex_plus_core::settings::RelayProfile::default();
        other.id = "keep-provider".to_string();
        other.name = "Keep Provider".to_string();
        settings.relay_profiles = vec![other];
        store.save(&settings).unwrap();

        let payload = apply_profile_with(
            CliproxyApplyRequest {
                channel: CHANNEL_OFFICIAL.to_string(),
                mode: String::new(),
                model: "account-2/gpt-5.4".to_string(),
                models: vec![
                    "gpt-5.4".to_string(),
                    "account-2/gpt-5.4".to_string(),
                    "anthropic/claude-sonnet-4".to_string(),
                ],
            },
            &layout,
            &store,
        )
        .unwrap();

        assert!(payload.created);
        assert!(payload.settings.official_login_mixed_mode);
        assert!(
            payload
                .settings
                .relay_profiles
                .iter()
                .any(|profile| profile.id == "keep-provider")
        );
        let managed = payload
            .settings
            .relay_profiles
            .iter()
            .find(|profile| profile.id == OFFICIAL_PROFILE_ID)
            .unwrap();
        assert_eq!(managed.integration_type, OFFICIAL_INTEGRATION_TYPE);
        assert_eq!(managed.relay_mode, RelayMode::PureApi);
        assert_eq!(managed.base_url, "http://127.0.0.1:9123/v1");
        assert!(managed.config_contents.contains("http://127.0.0.1:9123/v1"));
        assert!(managed.model_list.contains("account-2/gpt-5.4"));
        assert!(!managed.model_list.contains("claude-sonnet-4"));
    }

    #[test]
    fn integration_channel_keeps_all_models_and_coexists_with_official_channel() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::new(temp.path().join("CLIProxyAPI"));
        prepare_directories(&layout).unwrap();
        fs::write(&layout.config_path, "host: 127.0.0.1\nport: 8317\n").unwrap();
        save_secrets(
            &layout,
            &CliproxySecrets {
                version: 1,
                api_key: "test-api-key".to_string(),
                management_key: "test-management-key".to_string(),
            },
        )
        .unwrap();
        let store = SettingsStore::new(temp.path().join("settings.json"));
        let mut settings = BackendSettings::default();
        let mut official = codex_plus_core::settings::RelayProfile::default();
        official.id = OFFICIAL_PROFILE_ID.to_string();
        official.integration_type = OFFICIAL_INTEGRATION_TYPE.to_string();
        official.model_list = "gpt-5.4".to_string();
        settings.relay_profiles.push(official);
        store.save(&settings).unwrap();

        let request = CliproxyApplyRequest {
            channel: CHANNEL_GENERAL.to_string(),
            mode: String::new(),
            model: "anthropic/claude-sonnet-4".to_string(),
            models: vec![
                "gpt-5.4".to_string(),
                "account-2/gpt-5.4".to_string(),
                "anthropic/claude-sonnet-4".to_string(),
                "gemini-2.5-pro".to_string(),
            ],
        };
        let payload = apply_profile_with(request, &layout, &store).unwrap();
        assert_eq!(
            payload
                .settings
                .relay_profiles
                .iter()
                .filter(|profile| is_managed_profile(profile))
                .count(),
            2
        );
        let general = payload
            .settings
            .relay_profiles
            .iter()
            .find(|profile| profile.id == GENERAL_PROFILE_ID)
            .unwrap();
        assert_eq!(general.integration_type, GENERAL_INTEGRATION_TYPE);
        assert_eq!(general.name, "CLIProxyAPI");
        assert!(general.model_list.contains("gpt-5.4"));
        assert!(general.model_list.contains("account-2/gpt-5.4"));
        assert!(general.model_list.contains("claude-sonnet-4"));
        assert!(general.model_list.contains("gemini-2.5-pro"));
    }

    #[test]
    fn disabling_official_channel_keeps_general_channel_and_restores_active_profile() {
        let temp = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(temp.path().join("settings.json"));
        let mut settings = BackendSettings::default();
        settings.active_relay_id = OFFICIAL_PROFILE_ID.to_string();
        settings.relay_profiles = vec![
            codex_plus_core::settings::RelayProfile {
                id: OFFICIAL_PROFILE_ID.to_string(),
                integration_type: OFFICIAL_INTEGRATION_TYPE.to_string(),
                ..codex_plus_core::settings::RelayProfile::default()
            },
            codex_plus_core::settings::RelayProfile {
                id: GENERAL_PROFILE_ID.to_string(),
                integration_type: GENERAL_INTEGRATION_TYPE.to_string(),
                ..codex_plus_core::settings::RelayProfile::default()
            },
        ];
        store.save(&settings).unwrap();

        let payload = remove_official_profile_with(&store).unwrap();

        assert!(
            payload
                .settings
                .relay_profiles
                .iter()
                .all(|profile| !is_official_managed_profile(profile))
        );
        assert!(
            payload
                .settings
                .relay_profiles
                .iter()
                .any(is_general_managed_profile)
        );
        assert_eq!(payload.settings.active_relay_id, GENERAL_PROFILE_ID);
    }

    #[test]
    fn disabling_integration_removes_both_managed_profiles_and_restores_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(temp.path().join("settings.json"));
        let mut settings = BackendSettings::default();
        settings.active_relay_id = GENERAL_PROFILE_ID.to_string();
        settings.relay_profiles = vec![
            codex_plus_core::settings::RelayProfile {
                id: "fallback".to_string(),
                ..codex_plus_core::settings::RelayProfile::default()
            },
            codex_plus_core::settings::RelayProfile {
                id: GENERAL_PROFILE_ID.to_string(),
                integration_type: GENERAL_INTEGRATION_TYPE.to_string(),
                ..codex_plus_core::settings::RelayProfile::default()
            },
            codex_plus_core::settings::RelayProfile {
                id: OFFICIAL_PROFILE_ID.to_string(),
                integration_type: OFFICIAL_INTEGRATION_TYPE.to_string(),
                ..codex_plus_core::settings::RelayProfile::default()
            },
        ];
        store.save(&settings).unwrap();

        let payload = remove_integration_profiles_with(&store).unwrap();

        assert!(
            payload
                .settings
                .relay_profiles
                .iter()
                .all(|profile| !is_managed_profile(profile))
        );
        assert_eq!(payload.settings.active_relay_id, "fallback");
    }

    #[test]
    fn cliproxy_model_classification_accepts_account_prefixed_official_models() {
        assert!(cliproxy_model_is_official("gpt-5.6-sol"));
        assert!(cliproxy_model_is_official("account-2/gpt-5.4"));
        assert!(!cliproxy_model_is_official("anthropic/claude-sonnet-4"));
        assert!(!cliproxy_model_is_official("openai/gpt-4.1"));
    }
}

impl PartialEq for CliproxySecrets {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
            && self.api_key == other.api_key
            && self.management_key == other.management_key
    }
}
