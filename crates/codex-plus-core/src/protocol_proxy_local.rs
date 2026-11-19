// 本地官方账号、旧聚合及渠道保护兼容路径；共享外层代理端口与进程生命周期。
//! Codex Responses API 与 OpenAI Chat Completions 的本地协议转换。
//!
//! Codex Chat 与 Responses 协议之间的转换实现。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::pin::Pin;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use futures_util::{Stream, StreamExt};
use serde_json::{Map, Value, json};

use crate::relay_rotation::{RotationContext, RotationEvent};
use crate::settings::{RelayProtocol, SettingsStore};

pub const DEFAULT_PROTOCOL_PROXY_PORT: u16 = 57321;
pub const OFFICIAL_CHATGPT_CODEX_RESPONSES_URL: &str =
    "https://chatgpt.com/backend-api/codex/responses";
pub const OFFICIAL_CHATGPT_CODEX_IMAGE_GENERATIONS_URL: &str =
    "https://chatgpt.com/backend-api/codex/images/generations";
pub const OFFICIAL_CHATGPT_CODEX_IMAGE_EDITS_URL: &str =
    "https://chatgpt.com/backend-api/codex/images/edits";

/// 协议代理实际生效端口，默认 DEFAULT_PROTOCOL_PROXY_PORT。
pub fn protocol_proxy_port() -> u16 {
    super::protocol_proxy_port()
}

pub const NO_AUTH_PROXY_BEARER_TOKEN: &str = "codex-plus-no-auth";
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const UPSTREAM_HEADER_TIMEOUT: Duration = Duration::from_secs(30);
const UPSTREAM_STREAM_HEADER_TIMEOUT: Duration = Duration::from_secs(120);
const UPSTREAM_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const UPSTREAM_BODY_TIMEOUT: Duration = Duration::from_secs(120);
const UPSTREAM_IMAGE_HEADER_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_COOLDOWN_RETRIES: usize = 3;
const THINK_OPEN_TAG: &str = "<think>";
const THINK_CLOSE_TAG: &str = "</think>";
const EXTRA_CHAT_PASSTHROUGH_FIELDS: &[&str] = &[
    "frequency_penalty",
    "logit_bias",
    "logprobs",
    "metadata",
    "n",
    "presence_penalty",
    "response_format",
    "seed",
    "service_tier",
    "stop",
    "stream_options",
    "top_logprobs",
    "user",
];
const ERROR_BODY_PREVIEW_LIMIT: usize = 1024;
const STREAM_CAPACITY_PROBE_LIMIT: usize = 8 * 1024 * 1024;
const CAPACITY_RETRY_TRACKER_TTL: Duration = Duration::from_secs(300);
const CAPACITY_RETRY_TRACKER_MAX_KEYS: usize = 1024;
const CAPACITY_RETRY_BACKOFF_BASE: Duration = Duration::from_millis(250);

#[derive(Debug)]
struct CapacityRetryAttemptState {
    attempts: u8,
    last_seen: Instant,
}

static CAPACITY_RETRY_ATTEMPTS: OnceLock<Mutex<HashMap<u64, CapacityRetryAttemptState>>> =
    OnceLock::new();

#[derive(Debug, Clone)]
struct CapacityRetryNoticeState {
    sequence: u64,
    phase: &'static str,
    attempt: u8,
    max_attempts: u8,
    last_retry_at_ms: u64,
    updated_at_ms: u64,
}

impl Default for CapacityRetryNoticeState {
    fn default() -> Self {
        Self {
            sequence: 0,
            phase: "idle",
            attempt: 0,
            max_attempts: 0,
            last_retry_at_ms: 0,
            updated_at_ms: 0,
        }
    }
}

static CAPACITY_RETRY_NOTICE: OnceLock<Mutex<CapacityRetryNoticeState>> = OnceLock::new();

fn should_retry_after_cooldown(retries: usize) -> bool {
    retries < MAX_COOLDOWN_RETRIES
}

/// codex v2 远程压缩请求在 input 末尾携带的控制 item（openai/codex compact_remote_v2）。
const COMPACTION_TRIGGER_TYPE: &str = "compaction_trigger";
/// codex 期望响应里恰好包含一个的压缩结果 item，`encrypted_content` 只透传不校验。
const COMPACTION_OUTPUT_TYPE: &str = "compaction";
/// 本地代理生成摘要时注入的 user 指令（对齐 openai/codex prompts/templates/compact/prompt.md）。
const COMPACTION_SUMMARY_INSTRUCTION: &str = "You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task.\n\nInclude:\n- Current progress and key decisions made\n- Important context, constraints, or user preferences\n- What remains to be done (clear next steps)\n- Any critical data, examples, or references needed to continue\n\nBe concise, structured, and focused on helping the next LLM seamlessly continue the work.";
/// 历史回放时 `compaction` item 展开成的文本前缀（对齐 codex SUMMARY_PREFIX 语义）。
const COMPACTION_REPLAY_PREFIX: &str = "Another language model started to solve this problem and produced a summary of its thinking process. Here is the summary produced by the other language model:\n";

// 两条传输路径复用同一控制项识别，trigger 不在末尾时也不能漏过原生探测。
pub fn request_has_compaction_trigger(body: &Value) -> bool {
    super::request_has_compaction_trigger(body)
}

fn strip_compaction_trigger(body: Value) -> Value {
    super::strip_compaction_trigger(body)
}

/// 把压缩摘要请求改写成上游能理解的普通生成请求：
/// - input 末尾注入 user 摘要指令；
/// - tools/parallel_tool_calls 清空，避免摘要阶段触发工具调用。
fn rewrite_request_for_compaction(mut body: Value) -> Value {
    if let Some(items) = body.get_mut("input").and_then(Value::as_array_mut) {
        items.push(json!({
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": COMPACTION_SUMMARY_INSTRUCTION }]
        }));
    }
    body["tools"] = json!([]);
    body["tool_choice"] = json!("none");
    body["parallel_tool_calls"] = json!(false);
    body
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatReasoningStyle {
    Default,
    DeepSeek,
    LowHigh,
    OpenRouter,
    Thinking,
    EnableThinking,
    ReasoningSplit,
}

#[derive(Debug, Clone, Default)]
struct CodexToolContext {
    custom_tools: BTreeMap<String, CodexCustomToolSpec>,
    function_tools: BTreeMap<String, CodexFunctionToolSpec>,
    has_custom_tools: bool,
    has_namespace_tools: bool,
}

#[derive(Debug, Clone)]
struct CodexCustomToolSpec {
    openai_name: String,
    kind: CodexCustomToolKind,
    proxy_action: Option<CodexPatchProxyAction>,
}

#[derive(Debug, Clone, Default)]
struct CodexFunctionToolSpec {
    namespace: String,
    name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexCustomToolKind {
    Raw,
    ApplyPatch,
    BuiltIn,
    /// Codex 的 tool_search 工具（MCP 延迟加载检索，execution: client）。
    ToolSearch,
}

impl Default for CodexCustomToolKind {
    fn default() -> Self {
        Self::Raw
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexPatchProxyAction {
    AddFile,
    DeleteFile,
    UpdateFile,
    ReplaceFile,
    Batch,
}

impl CodexPatchProxyAction {
    fn suffix(self) -> &'static str {
        match self {
            Self::AddFile => "add_file",
            Self::DeleteFile => "delete_file",
            Self::UpdateFile => "update_file",
            Self::ReplaceFile => "replace_file",
            Self::Batch => "batch",
        }
    }
}

impl CodexToolContext {
    fn is_custom_tool_proxy(&self, upstream_name: &str) -> bool {
        self.custom_tools.contains_key(upstream_name)
    }

    fn is_tool_search_proxy(&self, upstream_name: &str) -> bool {
        self.custom_tools.get(upstream_name).map(|spec| spec.kind)
            == Some(CodexCustomToolKind::ToolSearch)
    }

    fn original_custom_tool_name(&self, upstream_name: &str) -> String {
        self.custom_tools
            .get(upstream_name)
            .map(|spec| spec.openai_name.clone())
            .unwrap_or_else(|| upstream_name.to_string())
    }

    fn openai_name_for_function_tool(&self, upstream_name: &str) -> (String, String) {
        let Some(spec) = self.function_tools.get(upstream_name) else {
            return (upstream_name.to_string(), String::new());
        };
        let name = if spec.name.is_empty() {
            upstream_name.to_string()
        } else {
            spec.name.clone()
        };
        (name, spec.namespace.clone())
    }
}

pub fn local_responses_proxy_base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1")
}

pub fn responses_to_chat_completions(body: Value) -> anyhow::Result<Value> {
    responses_to_chat_completions_with_options(body, false)
}

pub fn responses_to_chat_completions_with_options(
    body: Value,
    standard: bool,
) -> anyhow::Result<Value> {
    let mut result = json!({});

    if let Some(model) = body.get("model") {
        result["model"] = model.clone();
    }

    let mut messages = Vec::new();
    if let Some(instructions) = body.get("instructions") {
        let text = instruction_text(instructions);
        if !text.is_empty() {
            messages.push(json!({ "role": "system", "content": text }));
        }
    }

    if let Some(input) = body.get("input") {
        append_responses_input(input, &mut messages);
    }
    // 必须在 enforce_tool_call_pairing 之前：配对一旦被错误摘除就无法恢复。
    relocate_interleaved_non_tool_messages(&mut messages);
    enforce_tool_call_pairing(&mut messages);
    // 配对判定之后仍有无主 tool 消息（上游会直接 400），降级成 user 保住内容。
    degrade_unpaired_tool_messages(&mut messages);
    // 必须在 enforce_tool_call_pairing 之后：它依赖 tool 消息的连续性，
    // 而这一步会往中间插入 user 消息。
    relocate_tool_output_images(&mut messages);
    ensure_tool_call_reasoning_content(&mut messages);
    normalize_chat_messages(&mut messages);
    let messages = collapse_system_messages_to_head(messages);
    result["messages"] = json!(messages);

    let model = body.get("model").and_then(Value::as_str).unwrap_or("");
    if let Some(value) = body.get("max_output_tokens") {
        if is_openai_o_series(model) {
            result["max_completion_tokens"] = value.clone();
        } else {
            result["max_tokens"] = value.clone();
        }
    }
    if let Some(value) = body.get("max_tokens") {
        result["max_tokens"] = value.clone();
    }
    if let Some(value) = body.get("max_completion_tokens") {
        result["max_completion_tokens"] = value.clone();
    }

    for key in ["temperature", "top_p", "stream"] {
        if let Some(value) = body.get(key) {
            result[key] = value.clone();
        }
    }
    if body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        let mut stream_options = body
            .get("stream_options")
            .cloned()
            .unwrap_or_else(|| json!({}));
        stream_options["include_usage"] = json!(true);
        result["stream_options"] = stream_options;
    }

    apply_chat_reasoning_options(&mut result, &body, model, standard);

    let mut tool_context = build_codex_tool_context(body.get("tools"));
    // Codex 客户端把 tool_search 命中的 MCP 命名空间挂在历史里的 tool_search_output
    // item 上，却不会把它们提升进下一轮请求的 tools 数组。这里补上这一跳：先登记进
    // tool context（模型调用回来时才能还原 namespace），再用既有 namespace 链路展开
    // 成 mcp__<server>__<tool>，否则上游只看到命名空间外壳，调不到内层工具。
    let harvested_namespaces = collect_tool_search_output_namespaces(&body);
    for namespace_tool in &harvested_namespaces {
        add_namespace_tools_to_context(&mut tool_context, namespace_tool);
    }
    let mut has_chat_tools = false;
    let mut converted = Vec::new();
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        converted = responses_tools_to_chat_tools(tools, &tool_context);
    }
    for namespace_tool in &harvested_namespaces {
        converted.extend(namespace_tool_to_chat_tools(namespace_tool, &tool_context));
    }
    dedup_chat_tools_by_name(&mut converted);
    if !converted.is_empty() {
        has_chat_tools = true;
        result["tools"] = json!(converted);
    }

    if has_chat_tools {
        if let Some(tool_choice) = body
            .get("tool_choice")
            .and_then(|value| responses_tool_choice_to_chat(value, &tool_context))
        {
            result["tool_choice"] = tool_choice;
        }
        if let Some(value) = body.get("parallel_tool_calls") {
            result["parallel_tool_calls"] = value.clone();
        }
    }

    for key in EXTRA_CHAT_PASSTHROUGH_FIELDS {
        if *key == "stream_options" && result.get("stream_options").is_some() {
            continue;
        }
        if let Some(value) = body.get(*key) {
            result[*key] = value.clone();
        }
    }

    Ok(result)
}

pub fn chat_completion_to_response(body: Value) -> anyhow::Result<Value> {
    chat_completion_to_response_with_context(body, &CodexToolContext::default(), None)
}

pub fn chat_completion_to_response_with_request(
    body: Value,
    original_request: &Value,
) -> anyhow::Result<Value> {
    let mut context = build_codex_tool_context(original_request.get("tools"));
    // 反向同样要认领 tool_search_output 里的命名空间，否则模型调用
    // mcp__<server>__<tool> 回来时查不到 namespace，还原不成 Responses item。
    for namespace_tool in &collect_tool_search_output_namespaces(original_request) {
        add_namespace_tools_to_context(&mut context, namespace_tool);
    }
    chat_completion_to_response_with_context(body, &context, Some(original_request))
}

fn chat_completion_to_response_with_context(
    body: Value,
    tool_context: &CodexToolContext,
    original_request: Option<&Value>,
) -> anyhow::Result<Value> {
    let choices = body
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("chat response missing choices"))?;
    let choice = choices
        .first()
        .ok_or_else(|| anyhow::anyhow!("chat response choices is empty"))?;
    let message = choice
        .get("message")
        .ok_or_else(|| anyhow::anyhow!("chat response choice missing message"))?;

    let response_id = response_id_from_chat_id(body.get("id").and_then(Value::as_str));
    let mut output = Vec::new();
    if let Some(reasoning) = chat_reasoning_to_response_output_item(message, &response_id) {
        output.push(reasoning);
    }
    if let Some(message) = chat_message_to_response_output_item(message, &response_id) {
        output.push(message);
    }
    output.extend(chat_tool_calls_to_response_output_items(
        message,
        tool_context,
    ));

    let mut response = json!({
        "id": response_id,
        "object": "response",
        "created_at": body.get("created").and_then(Value::as_u64).unwrap_or(0),
        "status": response_status(choice.get("finish_reason").and_then(Value::as_str)),
        "model": body.get("model").and_then(Value::as_str).unwrap_or(""),
        "output": output,
        "usage": chat_usage_to_responses_usage(body.get("usage"))
    });

    if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
        response["incomplete_details"] = json!({ "reason": "max_output_tokens" });
    }
    copy_response_request_fields(&mut response, original_request);

    Ok(response)
}

pub struct ProxyHttpResponse {
    pub status: String,
    pub content_type: String,
    pub body: Vec<u8>,
}

pub struct UpstreamProxyResponse {
    pub status_code: u16,
    pub content_type: String,
    pub is_stream: bool,
    pub wire_api: UpstreamWireApi,
    /// 底层流命中容量错误时，调用方应在读取 body 前重新请求。
    pub capacity_retryable: bool,
    /// 当前 Responses 请求是否启用容量错误重写。
    pub capacity_retry_enabled: bool,
    /// 用于容量重试去重的请求指纹。
    pub capacity_retry_key: Option<u64>,
    pub capacity_retry_max_attempts: u8,
    /// 已预读但未消费的响应字节。
    pub prefetched_chunk: Vec<u8>,
    /// 上游是 Chat Completions 的合成摘要，或 Responses 压缩请求，需要重组为 compaction。
    pub compaction: bool,
    pub native_compaction_passthrough: bool,
    pub request_trace: Option<crate::request_log::RequestTrace>,
    pub exec_adapted: bool,
    pub(crate) exec_outcome: Option<crate::responses_exec_transport::ExecOutcome>,

    pub response: reqwest::Response,
    pub scheduled_account_id: Option<String>,
    pub(crate) _channel_permit: Option<crate::channel_protection::ChannelPermit>,
}

/// 客户端会话头白名单；官方父子请求必须经原生父子关系验证后使用当前来源。
#[derive(Clone, Copy, Default)]
pub struct ProxySessionHeaders<'a> {
    pub session_id: Option<&'a str>,
    pub thread_id: Option<&'a str>,
    pub opencode_session: Option<&'a str>,
}

impl ProxySessionHeaders<'_> {
    async fn following_provider(
        self,
        settings: &crate::settings::BackendSettings,
        provider: &str,
    ) -> anyhow::Result<Option<String>> {
        if !settings.official_experience.follows_current_account()
            || !crate::official_child_routing::official_provider(provider)
        {
            return Ok(None);
        }
        if provider != crate::official_account_schedule::PROVIDER_ID
            && provider.starts_with(crate::official_experience::DIRECT_OFFICIAL_PROVIDER_PREFIX)
        {
            anyhow::ensure!(
                crate::official_accounts::OfficialAccountStore::default()
                    .list()?
                    .iter()
                    .any(|account| {
                        crate::official_experience::direct_official_provider_id(&account.id)
                            == provider
                    }),
                "官方账号来源 provider「{provider}」不存在，停止账号跟随"
            );
        }
        crate::official_child_routing::inherited_provider_with_native_metadata(
            &crate::codex_home::default_codex_home_dir(),
            self.thread_id,
            self.session_id,
            provider,
        )
        .await
    }

    fn apply(
        self,
        mut request: reqwest::RequestBuilder,
        relay: Option<&crate::settings::RelayProfile>,
    ) -> reqwest::RequestBuilder {
        for (name, value) in [
            ("session-id", self.session_id),
            ("thread-id", self.thread_id),
            ("x-opencode-session", self.opencode_session),
        ] {
            if relay.is_some_and(|relay| {
                relay
                    .custom_headers
                    .iter()
                    .any(|header| header.key.trim().eq_ignore_ascii_case(name))
            }) {
                continue;
            }
            if let Some(value) = value
                .map(str::trim)
                .filter(|value| !value.is_empty() && value.len() <= 1024)
                .and_then(|value| reqwest::header::HeaderValue::from_str(value).ok())
            {
                request = request.header(name, value);
            }
        }
        request
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum UpstreamWireApi {
    Responses,
    AnthropicMessages,
    ChatCompletions,
    AudioTranscriptions,
    ImageGenerations,
    ImageEdits,
}

#[derive(Debug, Clone)]
struct ModelRouteSelection {
    relay: crate::settings::RelayProfile,
    source_relay_id: String,
    source_model: String,
    upstream_model: String,
}

impl UpstreamProxyResponse {
    pub fn status(&self) -> String {
        http_status_line(self.status_code)
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status_code)
    }
}

fn capacity_retry_request_key(request: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    serde_json::to_vec(request)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

pub fn next_capacity_retry_attempt(request_key: u64, max_attempts: u8) -> Option<u8> {
    let now = Instant::now();
    let attempts = CAPACITY_RETRY_ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut attempts = attempts
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    attempts.retain(|_, state| now.duration_since(state.last_seen) <= CAPACITY_RETRY_TRACKER_TTL);
    if attempts.len() >= CAPACITY_RETRY_TRACKER_MAX_KEYS && !attempts.contains_key(&request_key) {
        if let Some(oldest_key) = attempts
            .iter()
            .min_by_key(|(_, state)| state.last_seen)
            .map(|(key, _)| *key)
        {
            attempts.remove(&oldest_key);
        }
    }
    let attempt = {
        let state = attempts
            .entry(request_key)
            .or_insert(CapacityRetryAttemptState {
                attempts: 0,
                last_seen: now,
            });
        state.attempts = state.attempts.saturating_add(1);
        state.last_seen = now;
        state.attempts
    };
    if attempt > max_attempts {
        attempts.remove(&request_key);
        None
    } else {
        Some(attempt)
    }
}

pub fn reset_capacity_retry_attempts(request_key: u64) {
    let Some(attempts) = CAPACITY_RETRY_ATTEMPTS.get() else {
        return;
    };
    attempts
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&request_key);
}

fn capacity_retry_notice_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

/// Returns the latest capacity retry notification for the injected Codex UI.
/// The notification is out-of-band and is never written into the Responses stream.
pub fn capacity_retry_status() -> Value {
    let state = CAPACITY_RETRY_NOTICE
        .get_or_init(|| Mutex::new(CapacityRetryNoticeState::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    json!({
        "sequence": state.sequence,
        "phase": state.phase,
        "attempt": state.attempt,
        "maxAttempts": state.max_attempts,
        "lastRetryAtMs": state.last_retry_at_ms,
        "updatedAtMs": state.updated_at_ms,
    })
}

/// Records a capacity retry and returns the notification sequence owned by the request.
pub fn record_capacity_retry_notice(attempt: u8, max_attempts: u8) -> u64 {
    let now = capacity_retry_notice_now_ms();
    let mut state = CAPACITY_RETRY_NOTICE
        .get_or_init(|| Mutex::new(CapacityRetryNoticeState::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.sequence = state.sequence.wrapping_add(1).max(1);
    state.phase = "retrying";
    state.attempt = attempt.max(1);
    state.max_attempts = max_attempts.max(1);
    state.last_retry_at_ms = now;
    state.updated_at_ms = now;
    state.sequence
}

/// Marks the latest retry as recovered or exhausted without emitting a Codex error event.
pub fn finish_capacity_retry_notice(sequence: u64, recovered: bool) {
    let Some(notice) = CAPACITY_RETRY_NOTICE.get() else {
        return;
    };
    let mut state = notice
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.sequence != sequence {
        return;
    }
    state.phase = if recovered { "recovered" } else { "exhausted" };
    state.updated_at_ms = capacity_retry_notice_now_ms();
}

fn capacity_retry_backoff(attempt: u8) -> Duration {
    let multiplier = u64::from(attempt.clamp(1, 8));
    CAPACITY_RETRY_BACKOFF_BASE
        .checked_mul(multiplier as u32)
        .unwrap_or(Duration::from_secs(2))
        .min(Duration::from_secs(2))
}

/// 在协议代理内部完成容量错误重试，不把中间的 503 暴露给 Codex。
///
/// Codex 原生客户端在部分官方登录路径收到容量错误后会直接结束任务，
/// 因此容量错误必须在本地代理内重新发起原始请求。达到配置上限后，
/// 底层请求函数会返回最后一次原始容量响应，由调用方决定如何展示。
pub async fn open_responses_proxy_request_with_capacity_retries(
    body: &str,
    original_user_agent: Option<&str>,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_with_capacity_retries_for_path(
        body,
        original_user_agent,
        "/responses",
    )
    .await
}

pub async fn open_responses_proxy_request_with_capacity_retries_for_path(
    body: &str,
    original_user_agent: Option<&str>,
    request_path: &str,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_with_capacity_retries_for_path_and_beta(
        body,
        original_user_agent,
        request_path,
        None,
    )
    .await
}

pub async fn open_responses_proxy_request_with_capacity_retries_for_path_and_beta(
    body: &str,
    original_user_agent: Option<&str>,
    request_path: &str,
    beta_features: Option<&str>,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_with_capacity_retries_for_path_and_session_headers(
        body,
        original_user_agent,
        request_path,
        beta_features,
        ProxySessionHeaders::default(),
    )
    .await
}

pub async fn open_responses_proxy_request_with_capacity_retries_for_path_and_session_headers(
    body: &str,
    original_user_agent: Option<&str>,
    request_path: &str,
    beta_features: Option<&str>,
    session_headers: ProxySessionHeaders<'_>,
) -> anyhow::Result<UpstreamProxyResponse> {
    let settings = SettingsStore::default().load().unwrap_or_default();
    open_responses_proxy_request_with_settings_and_capacity_retries_and_user_agent(
        body,
        settings,
        original_user_agent,
        request_path,
        OFFICIAL_CHATGPT_CODEX_RESPONSES_URL,
        beta_features,
        session_headers,
    )
    .await
}

#[doc(hidden)]
pub async fn open_responses_proxy_request_with_settings_and_capacity_retries(
    body: &str,
    settings: crate::settings::BackendSettings,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_with_settings_and_capacity_retries_and_user_agent(
        body,
        settings,
        None,
        "/responses",
        OFFICIAL_CHATGPT_CODEX_RESPONSES_URL,
        None,
        ProxySessionHeaders::default(),
    )
    .await
}

#[doc(hidden)]
pub async fn open_responses_proxy_request_with_settings_and_capacity_retries_and_official_endpoint(
    body: &str,
    settings: crate::settings::BackendSettings,
    official_endpoint: &str,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_with_settings_and_capacity_retries_and_user_agent(
        body,
        settings,
        None,
        "/responses",
        official_endpoint,
        None,
        ProxySessionHeaders::default(),
    )
    .await
}

pub(super) async fn open_responses_proxy_request_with_settings_and_capacity_retries_and_user_agent(
    body: &str,
    settings: crate::settings::BackendSettings,
    original_user_agent: Option<&str>,
    request_path: &str,
    official_endpoint: &str,
    beta_features: Option<&str>,
    session_headers: ProxySessionHeaders<'_>,
) -> anyhow::Result<UpstreamProxyResponse> {
    let mut retry_attempt = 0u8;
    let mut notice_sequence = None;
    loop {
        let upstream =
            match open_responses_proxy_request_with_settings_and_user_agent_and_official_endpoint(
                body,
                settings.clone(),
                original_user_agent,
                request_path,
                official_endpoint,
                beta_features,
                session_headers,
            )
            .await
            {
                Ok(upstream) => upstream,
                Err(error) => {
                    if let Some(sequence) = notice_sequence {
                        finish_capacity_retry_notice(sequence, false);
                    }
                    return Err(error);
                }
            };
        if !upstream.capacity_retryable {
            if let Some(sequence) = notice_sequence {
                let recovered = upstream.is_success()
                    && !is_selected_model_capacity_error(&upstream.prefetched_chunk);
                finish_capacity_retry_notice(sequence, recovered);
            }
            return Ok(upstream);
        }

        retry_attempt = retry_attempt.saturating_add(1);
        let max_attempts = upstream.capacity_retry_max_attempts.max(1);
        notice_sequence = Some(record_capacity_retry_notice(retry_attempt, max_attempts));
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "protocol_proxy.capacity_retry_loop",
            json!({
                "attempt": retry_attempt,
                "maxAttempts": max_attempts,
                "willRetry": true,
                "reason": "selected_model_at_capacity"
            }),
        );
        drop(upstream);
        tokio::time::sleep(capacity_retry_backoff(retry_attempt)).await;

        // The low-level request removes the tracker after max_attempts and
        // returns the original capacity response. This guard only protects
        // against malformed responses that do not carry a retry key.
        if u16::from(retry_attempt) > u16::from(max_attempts) + 1 {
            return open_responses_proxy_request_with_settings_and_user_agent_and_official_endpoint(
                body,
                settings.clone(),
                original_user_agent,
                request_path,
                official_endpoint,
                beta_features,
                session_headers,
            )
            .await;
        }
    }
}

#[cfg(test)]
#[test]
fn capacity_retry_attempts_stop_rewriting_after_the_configured_limit() {
    let request_key = u64::MAX;
    reset_capacity_retry_attempts(request_key);

    assert_eq!(next_capacity_retry_attempt(request_key, 2), Some(1));
    assert_eq!(next_capacity_retry_attempt(request_key, 2), Some(2));
    assert_eq!(next_capacity_retry_attempt(request_key, 2), None);
    assert_eq!(next_capacity_retry_attempt(request_key, 2), Some(1));

    reset_capacity_retry_attempts(request_key);
}

#[cfg(test)]
#[test]
fn capacity_probe_waits_past_responses_preamble_events() {
    let preamble = br#"event: response.created
data: {"type":"response.created"}

event: response.in_progress
data: {"type":"response.in_progress"}

"#;
    assert!(!stream_prefix_contains_normal_progress(preamble));

    let mut capacity_after_preamble = preamble.to_vec();
    capacity_after_preamble.extend_from_slice(
        br#"event: error
data: {"error":{"message":"Selected model is at capacity. Please try a different model."}}

"#,
    );
    assert!(is_selected_model_capacity_error(&capacity_after_preamble));

    let mut output_after_preamble = preamble.to_vec();
    output_after_preamble.extend_from_slice(
        br#"event: response.output_item.added
data: {"type":"response.output_item.added"}

"#,
    );
    assert!(!stream_prefix_contains_normal_progress(
        &output_after_preamble
    ));
    let mut capacity_after_output_item = output_after_preamble.clone();
    capacity_after_output_item.extend_from_slice(
        br#"event: error
data: {"error":{"message":"Selected model is at capacity. Please try a different model."}}

"#,
    );
    assert!(is_selected_model_capacity_error(
        &capacity_after_output_item
    ));
    output_after_preamble.extend_from_slice(
        br#"event: response.output_text.delta
data: {"type":"response.output_text.delta","delta":"hello"}

"#,
    );
    assert!(stream_prefix_contains_normal_progress(
        &output_after_preamble
    ));
}

#[cfg(test)]
#[test]
fn capacity_detector_understands_structured_and_escaped_errors() {
    let response_failed = br#"event: response.failed
data: {"type":"response.failed","response":{"status":"failed","error":{"code":"model_at_capacity","message":"temporarily unavailable"}}}

"#;
    assert!(is_selected_model_capacity_error(response_failed));

    let escaped_message = br#"{"error":{"type":"server_error","message":"Selected model is at capacit\u0079. Please try a different model."}}"#;
    assert!(is_selected_model_capacity_error(escaped_message));

    let incomplete_error_block = br#"event: response.created
data: {"type":"response.created"}

event: error
data: {"type":"error","code":"model_capacity_exceeded"}"#;
    assert!(is_selected_model_capacity_error(incomplete_error_block));
}

#[cfg(test)]
#[test]
fn capacity_detector_does_not_treat_output_or_other_failures_as_capacity() {
    let quoted_output = br#"event: response.output_text.delta
data: {"type":"response.output_text.delta","delta":"Selected model is at capacity"}

"#;
    assert!(!is_selected_model_capacity_error(quoted_output));

    let ordinary_failure = br#"event: response.failed
data: {"type":"response.failed","response":{"status":"failed","error":{"code":"server_error","message":"Internal server error"}}}

"#;
    assert!(!is_selected_model_capacity_error(ordinary_failure));
}

pub fn upstream_header_timeout() -> Duration {
    UPSTREAM_HEADER_TIMEOUT
}

pub fn upstream_stream_header_timeout() -> Duration {
    UPSTREAM_STREAM_HEADER_TIMEOUT
}

pub fn upstream_stream_idle_timeout() -> Duration {
    UPSTREAM_STREAM_IDLE_TIMEOUT
}

pub fn upstream_body_timeout() -> Duration {
    UPSTREAM_BODY_TIMEOUT
}

pub fn upstream_image_body_timeout() -> Duration {
    UPSTREAM_IMAGE_HEADER_TIMEOUT
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamChunkWaitError {
    IdleTimeout,
}

pub async fn next_stream_chunk_with_timeout<S>(
    stream: Pin<&mut S>,
    timeout: Duration,
) -> Result<Option<S::Item>, StreamChunkWaitError>
where
    S: Stream + ?Sized,
{
    let mut stream = stream;
    tokio::time::timeout(timeout, stream.next())
        .await
        .map_err(|_| StreamChunkWaitError::IdleTimeout)
}

pub async fn read_upstream_body_with_timeout(
    response: reqwest::Response,
    timeout: Duration,
) -> anyhow::Result<Vec<u8>> {
    let body = tokio::time::timeout(timeout, response.bytes())
        .await
        .with_context(|| format!("上游响应体超过 {} 秒未读取完成", timeout.as_secs()))?
        .context("读取上游响应体失败")?;
    Ok(body.to_vec())
}

pub fn upstream_http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(UPSTREAM_CONNECT_TIMEOUT)
        .user_agent("CodexPlusPlus/ProtocolProxy")
        .build()
        .context("failed to build upstream HTTP client")
}

pub async fn send_upstream_request(
    request: reqwest::RequestBuilder,
) -> anyhow::Result<reqwest::Response> {
    send_upstream_request_with_header_timeout(request, UPSTREAM_HEADER_TIMEOUT).await
}

pub async fn send_upstream_request_for_responses(
    request: reqwest::RequestBuilder,
    is_stream: bool,
) -> anyhow::Result<reqwest::Response> {
    let timeout = response_header_timeout(is_stream);
    send_upstream_request_with_header_timeout(request, timeout).await
}

pub async fn send_upstream_request_with_header_timeout(
    request: reqwest::RequestBuilder,
    timeout: Duration,
) -> anyhow::Result<reqwest::Response> {
    tokio::time::timeout(timeout, request.send())
        .await
        .with_context(|| format!("上游请求超过 {} 秒未返回响应头", timeout.as_secs()))?
        .context("上游请求失败")
}

pub struct ChatSseToResponsesConverter {
    buffer: String,
    utf8_remainder: Vec<u8>,
    state: ChatSseState,
    failed: bool,
}

/// codex v2 远程压缩的响应包装器：把上游摘要文本（无论 Responses 还是
/// Chat 上游、流式还是非流式）封装成「恰好一个 `compaction` 输出项」的
/// Responses SSE 流。输出项通过 output_item.done 显式交付给客户端，
/// 不能只放进 response.completed.response.output。
pub struct CompactionSseConverter {
    response_id: String,
    compaction_id: String,
    model: String,
    summary: String,
    final_summary: Option<String>,
    failed: Option<(String, Option<String>)>,
    /// 跨网络 chunk 攒 SSE 事件的缓冲（见 push_upstream_bytes）。
    sse_buffer: String,
    sse_utf8_remainder: Vec<u8>,
    responses_wire: bool,
    upstream_terminal: bool,
}

impl CompactionSseConverter {
    pub fn new(model: &str) -> Self {
        Self {
            response_id: format!("resp_compact_{}", chrono_now_millis()),
            compaction_id: format!("cmp_{}", uuid::Uuid::new_v4().simple()),
            model: model.to_string(),
            summary: String::new(),
            final_summary: None,
            failed: None,
            sse_buffer: String::new(),
            sse_utf8_remainder: Vec::new(),
            responses_wire: true,
            upstream_terminal: false,
        }
    }

    /// 标记上游是 Chat Completions（SSE chunk 的增量在 `choices[].delta.content`）。
    /// Responses 上游默认，增量在 `response.output_text.delta` 事件里。
    pub fn with_chat_upstream(mut self) -> Self {
        self.responses_wire = false;
        self
    }

    /// 追加上游输出的一块文本内容（非流式路径直接喂完整摘要）。
    pub fn push_summary_text(&mut self, text: &str) {
        self.summary.push_str(text);
    }

    /// 当前已收集的原始摘要文本（剥 think 之前），供空摘要判定。
    pub fn summary_text(&self) -> &str {
        &self.summary
    }

    pub fn has_upstream_terminal(&self) -> bool {
        self.upstream_terminal
    }

    /// 喂入上游流式响应的一个网络 chunk。SSE 事件可能被 TCP 拆开，
    /// 内部按 `\n\n` 边界缓冲；残缺块留在缓冲区等下一个 chunk。
    pub fn push_upstream_bytes(&mut self, bytes: &[u8]) {
        append_utf8_safe(&mut self.sse_buffer, &mut self.sse_utf8_remainder, bytes);
        while let Some(block) = take_sse_block(&mut self.sse_buffer) {
            if block.trim().is_empty() {
                continue;
            }
            self.handle_upstream_sse_block(&block);
        }
    }

    fn handle_upstream_sse_block(&mut self, block: &str) {
        let mut event_name = "";
        let mut data_parts: Vec<&str> = Vec::new();
        for line in block.lines() {
            if let Some(event) = strip_sse_field(line, "event") {
                event_name = event.trim();
            }
            if let Some(data) = strip_sse_field(line, "data") {
                data_parts.push(data);
            }
        }
        if data_parts.is_empty() {
            return;
        }
        let data = data_parts.join("\n");
        if data.trim() == "[DONE]" {
            self.upstream_terminal = true;
            return;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(&data) else {
            return;
        };
        let event_type = chunk
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(event_name);
        self.upstream_terminal |= is_terminal_responses_event(event_type)
            || chunk
                .get("choices")
                .and_then(Value::as_array)
                .is_some_and(|choices| {
                    choices.iter().any(|choice| {
                        choice
                            .get("finish_reason")
                            .is_some_and(|reason| !reason.is_null())
                    })
                });
        if matches!(
            event_type,
            "response.failed" | "response.incomplete" | "error"
        ) || chunk.get("error").is_some_and(|error| !error.is_null())
        {
            let error = chunk
                .pointer("/response/error")
                .or_else(|| chunk.get("error"));
            self.fail(
                error
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Compaction upstream stream failed or was incomplete")
                    .to_string(),
                error
                    .and_then(|error| error.get("code"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
            );
            return;
        }
        if self.responses_wire {
            match event_type {
                // Responses 流优先读取正文增量，排除携带同名 delta 字段的推理事件。
                "response.output_text.delta" => {
                    if let Some(delta) = chunk.get("delta").and_then(Value::as_str) {
                        self.summary.push_str(delta);
                    }
                }
                // 部分兼容实现不发送 delta，只在完成事件里携带完整正文。
                "response.output_text.done" => {
                    self.capture_final_summary(chunk.get("text"));
                }
                "response.content_part.done" => {
                    self.capture_final_summary(chunk.pointer("/part/text"));
                }
                "response.output_item.done" => {
                    let text = chunk
                        .get("item")
                        .map(extract_summary_text_from_response_item)
                        .unwrap_or_default();
                    self.capture_final_summary(Some(&json!(text)));
                }
                "response.completed" => {
                    let text = chunk
                        .get("response")
                        .map(extract_summary_text_from_responses)
                        .unwrap_or_default();
                    self.capture_final_summary(Some(&json!(text)));
                }
                _ => {}
            }
        } else if let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        {
            if let Some(content) = choice
                .get("delta")
                .and_then(|delta| delta.get("content"))
                .and_then(Value::as_str)
            {
                self.summary.push_str(content);
            }
            self.capture_final_summary(
                choice
                    .get("message")
                    .and_then(|message| message.get("content")),
            );
        }
    }

    fn capture_final_summary(&mut self, text: Option<&Value>) {
        let Some(text) = text
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
        else {
            return;
        };
        self.final_summary = Some(text.to_string());
    }

    pub fn fail(&mut self, message: String, error_type: Option<String>) -> Vec<u8> {
        self.failed = Some((message, error_type));
        Vec::new()
    }

    /// 收尾：产出完整的 compaction 响应 SSE。
    /// 链式推理上游（DeepSeek thinking 等）会把推理过程以 `<think>` 块
    /// 混进正文，这里统一剥掉首部完整 think 块，只保留真正的摘要答案；
    /// think 块未闭合（上游截断）时整个丢弃——剩下的只有推理残片。
    pub fn finish(mut self) -> Vec<u8> {
        if let Some(final_summary) = self.final_summary.take() {
            self.summary = final_summary;
        }
        if let Some((_reasoning, answer)) = split_leading_think_block(&self.summary) {
            self.summary = answer;
        } else if self.summary.trim_start().starts_with(THINK_OPEN_TAG) {
            self.summary = String::new();
        }
        self.summary = self.summary.trim().to_string();
        // 剥掉 think 块后为空（上游空输出、纯推理文本、截断导致块未闭合），
        // 一律按失败返回，避免向 codex 交付 `completed + 空 encrypted_content`
        // 的空 checkpoint 静默清空会话。
        if self.failed.is_none() && self.summary.is_empty() {
            self.failed = Some((
                "上游返回了空摘要，无法完成压缩".to_string(),
                Some("compaction_empty_summary".to_string()),
            ));
        }
        let mut output = String::new();
        let mut response = json!({
            "id": self.response_id,
            "object": "response",
            "created_at": chrono_now_millis() / 1000,
            "status": "in_progress",
            "model": self.model,
            "output": [],
            "usage": default_responses_usage()
        });
        push_sse(
            &mut output,
            "response.created",
            json!({"type": "response.created", "sequence_number": 0, "response": response}),
        );
        if let Some((message, error_type)) = self.failed {
            response["status"] = json!("failed");
            response["error"] = json!({
                "code": error_type.unwrap_or_else(|| "compaction_failed".to_string()),
                "message": message
            });
            push_sse(
                &mut output,
                "response.failed",
                json!({"type": "response.failed", "sequence_number": 1, "response": response}),
            );
            output.push_str("data: [DONE]\n\n");
            return output.into_bytes();
        }
        let compaction_item = json!({
            "id": self.compaction_id,
            "type": COMPACTION_OUTPUT_TYPE,
            "encrypted_content": self.summary
        });
        for (sequence_number, event_type) in [
            (1, "response.output_item.added"),
            (2, "response.output_item.done"),
        ] {
            push_sse(
                &mut output,
                event_type,
                json!({
                    "type": event_type,
                    "sequence_number": sequence_number,
                    "output_index": 0,
                    "item": compaction_item
                }),
            );
        }
        response["status"] = json!("completed");
        response["output"] = json!([compaction_item]);
        push_sse(
            &mut output,
            "response.completed",
            json!({
                "type": "response.completed",
                "sequence_number": 3,
                "response": response
            }),
        );
        output.push_str("data: [DONE]\n\n");
        output.into_bytes()
    }
}

fn chrono_now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 非流式压缩响应包装：从上游 JSON 响应里提取 assistant 文本并封装成
/// 恰好一个 `compaction` 输出项的 Responses 响应。
pub fn wrap_non_stream_response_as_compaction(
    upstream_body: &[u8],
    model: &str,
) -> anyhow::Result<Vec<u8>> {
    let upstream_json: Value = serde_json::from_slice(upstream_body)?;
    let mut converter = CompactionSseConverter::new(model);
    if let Some(error) = upstream_json.get("error").filter(|value| !value.is_null()) {
        converter.fail(
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("compaction upstream error")
                .to_string(),
            None,
        );
        return Ok(converter.finish());
    }
    let responses_text = extract_summary_text_from_responses(&upstream_json);
    let text = if responses_text.is_empty() {
        extract_summary_text_from_chat(&upstream_json)
    } else {
        responses_text
    };
    if text.is_empty() {
        converter.fail(
            "上游返回了空摘要，无法完成压缩".to_string(),
            Some("compaction_empty_summary".to_string()),
        );
        return Ok(converter.finish());
    }
    converter.push_summary_text(&text);
    Ok(converter.finish())
}

/// 从 Responses JSON 响应（`output[].content[].text`）提取 assistant 文本。
fn extract_summary_text_from_responses(response: &Value) -> String {
    let Some(items) = response.get("output").and_then(Value::as_array) else {
        return String::new();
    };
    let mut texts = Vec::new();
    for item in items {
        let text = extract_summary_text_from_response_item(item);
        if !text.is_empty() {
            texts.push(text);
        }
    }
    texts.join("\n")
}

fn extract_summary_text_from_response_item(item: &Value) -> String {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => item
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Some(COMPACTION_OUTPUT_TYPE) => item
            .get("encrypted_content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

/// 从 Chat Completions JSON 响应提取 assistant 文本。
fn extract_summary_text_from_chat(response: &Value) -> String {
    response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// 历史回放：把 codex 历史里的 `compaction` item 展开成明文 user 消息。
/// codex 下游请求会把上次压缩结果作为 `{"type":"compaction","encrypted_content":"..."}`
/// 放进 input，第三方模型看不懂该类型，必须转成文本。
fn expand_compaction_item(item: &Value) -> Option<Value> {
    let summary = item.get("encrypted_content").and_then(Value::as_str)?;
    let mut text = COMPACTION_REPLAY_PREFIX.to_string();
    text.push_str(summary);
    Some(json!({
        "role": "user",
        "content": text
    }))
}

impl Default for ChatSseToResponsesConverter {
    fn default() -> Self {
        Self {
            buffer: String::new(),
            utf8_remainder: Vec::new(),
            state: ChatSseState::default(),
            failed: false,
        }
    }
}

impl ChatSseToResponsesConverter {
    pub fn with_request(original_request: &Value) -> Self {
        Self {
            state: ChatSseState::with_request(original_request),
            ..Self::default()
        }
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) -> Vec<u8> {
        append_utf8_safe(&mut self.buffer, &mut self.utf8_remainder, bytes);
        let mut output = String::new();
        while let Some(block) = take_sse_block(&mut self.buffer) {
            if block.trim().is_empty() {
                continue;
            }
            self.handle_block(&block, &mut output);
            if self.failed {
                break;
            }
        }
        output.into_bytes()
    }

    pub fn finish(&mut self) -> Vec<u8> {
        if !self.utf8_remainder.is_empty() {
            self.buffer
                .push_str(&String::from_utf8_lossy(&self.utf8_remainder));
            self.utf8_remainder.clear();
        }

        let mut output = String::new();
        if !self.failed {
            if self.state.completed || self.state.finish_reason.is_some() {
                self.state.finalize_into(&mut output);
            } else {
                return self.fail(
                    "Upstream stream ended before a terminal Chat Completions event".to_string(),
                    Some("incomplete_stream".to_string()),
                );
            }
        }
        output.into_bytes()
    }

    pub fn fail(&mut self, message: String, error_type: Option<String>) -> Vec<u8> {
        let mut output = String::new();
        self.state.failed_into(&mut output, message, error_type);
        self.failed = true;
        output.into_bytes()
    }

    pub fn has_failed(&self) -> bool {
        self.failed
    }

    fn handle_block(&mut self, block: &str, output: &mut String) {
        let mut event_name: Option<String> = None;
        let mut data_parts = Vec::new();
        for line in block.lines() {
            if let Some(event) = strip_sse_field(line, "event") {
                event_name = Some(event.trim().to_string());
            }
            if let Some(data) = strip_sse_field(line, "data") {
                data_parts.push(data.to_string());
            }
        }

        if data_parts.is_empty() {
            return;
        }
        let data = data_parts.join("\n");
        if data.trim() == "[DONE]" {
            self.state.finalize_into(output);
            return;
        }

        let Ok(chunk) = serde_json::from_str::<Value>(&data) else {
            return;
        };
        if event_name.as_deref() == Some("error") || chunk.get("error").is_some() {
            let (message, error_type) = extract_chat_sse_error(&chunk);
            self.state.failed_into(output, message, error_type);
            self.failed = true;
            return;
        }
        self.state.handle_chat_chunk_into(&chunk, output);
    }
}

#[derive(Debug, Default)]
pub struct ResponsesSseTerminalTracker {
    buffer: String,
    utf8_remainder: Vec<u8>,
    terminal: bool,
    failed: bool,
    pub quota_reset: Option<i64>,
    pub usage: Option<crate::request_log::RequestUsage>,
}

impl ResponsesSseTerminalTracker {
    pub fn observe(&mut self, bytes: &[u8]) {
        if self.terminal {
            return;
        }
        append_utf8_safe(&mut self.buffer, &mut self.utf8_remainder, bytes);
        while let Some(block) = take_sse_block(&mut self.buffer) {
            self.observe_block(&block);
            if self.terminal {
                break;
            }
        }
    }

    pub fn finish(&mut self) {
        if self.terminal {
            return;
        }
        if !self.utf8_remainder.is_empty() {
            self.buffer
                .push_str(&String::from_utf8_lossy(&self.utf8_remainder));
            self.utf8_remainder.clear();
        }
        if !self.buffer.trim().is_empty() {
            let block = std::mem::take(&mut self.buffer);
            self.observe_block(&block);
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    pub fn has_failed(&self) -> bool {
        self.failed
    }

    fn observe_block(&mut self, block: &str) {
        self.quota_reset = self.quota_reset.or_else(|| {
            crate::official_account_schedule::quota_error_reset(
                block.as_bytes(),
                chrono::Utc::now().timestamp(),
            )
        });
        let mut event_name = None;
        let mut data_parts = Vec::new();
        for line in block.lines() {
            if let Some(event) = strip_sse_field(line, "event") {
                event_name = Some(event.trim());
            }
            if let Some(data) = strip_sse_field(line, "data") {
                data_parts.push(data);
            }
        }
        let data = data_parts.join("\n");
        if let Ok(value) = serde_json::from_str::<Value>(&data)
            && let Some(usage) = crate::request_log::usage_from_response_json(&value)
        {
            self.usage = Some(usage);
        }
        if event_name.is_some_and(is_terminal_responses_event) {
            self.terminal = true;
            self.failed = event_name != Some("response.completed");
            return;
        }
        if data.trim() == "[DONE]" {
            self.terminal = true;
            return;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&data)
            && value
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(is_terminal_responses_event)
        {
            self.terminal = true;
            self.failed = value.get("type").and_then(Value::as_str) != Some("response.completed");
        }
    }
}

fn is_terminal_responses_event(event: &str) -> bool {
    matches!(
        event,
        "response.completed" | "response.failed" | "response.incomplete"
    )
}

pub fn responses_stream_failure_events(
    original_request: Option<&Value>,
    message: String,
    error_type: Option<String>,
) -> Vec<u8> {
    let mut converter = original_request
        .map(ChatSseToResponsesConverter::with_request)
        .unwrap_or_default();
    converter.fail(message, error_type)
}

pub fn responses_stream_failure_from_upstream(
    original_request: Option<&Value>,
    status_code: u16,
    content_type: &str,
    body: &[u8],
) -> Vec<u8> {
    let (message, error_type, _, _) = upstream_error_parts(status_code, content_type, body);
    responses_stream_failure_events(original_request, message, error_type)
}

pub fn is_responses_proxy_path(path: &str) -> bool {
    let path = crate::official_experience::route_operation_path(path);
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    matches!(
        path,
        "/responses"
            | "/v1/responses"
            | "/v1/v1/responses"
            | "/codex/v1/responses"
            | "/responses/compact"
            | "/v1/responses/compact"
            | "/v1/v1/responses/compact"
            | "/codex/v1/responses/compact"
    )
}

pub fn is_responses_compact_proxy_path(path: &str) -> bool {
    let path = crate::official_experience::route_operation_path(path);
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    matches!(
        path,
        "/responses/compact"
            | "/v1/responses/compact"
            | "/v1/v1/responses/compact"
            | "/codex/v1/responses/compact"
    )
}

pub fn is_chat_completions_proxy_path(path: &str) -> bool {
    let path = crate::official_experience::route_operation_path(path);
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    matches!(
        path,
        "/chat/completions"
            | "/v1/chat/completions"
            | "/v1/v1/chat/completions"
            | "/codex/v1/chat/completions"
    )
}

pub fn is_models_proxy_path(path: &str) -> bool {
    let path = crate::official_experience::route_operation_path(path);
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    matches!(
        path,
        "/models" | "/v1/models" | "/v1/v1/models" | "/codex/v1/models"
    )
}

pub fn is_audio_transcriptions_proxy_path(path: &str) -> bool {
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    matches!(
        path,
        "/audio/transcriptions"
            | "/v1/audio/transcriptions"
            | "/v1/v1/audio/transcriptions"
            | "/codex/v1/audio/transcriptions"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProxyOperation {
    Generate,
    Edit,
}

impl ImageProxyOperation {
    fn name(self) -> &'static str {
        match self {
            Self::Generate => "generation",
            Self::Edit => "edit",
        }
    }

    fn official_endpoint(self) -> &'static str {
        match self {
            Self::Generate => OFFICIAL_CHATGPT_CODEX_IMAGE_GENERATIONS_URL,
            Self::Edit => OFFICIAL_CHATGPT_CODEX_IMAGE_EDITS_URL,
        }
    }
}

pub fn image_proxy_operation(path: &str) -> Option<ImageProxyOperation> {
    let path = crate::official_experience::route_operation_path(path);
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    match path {
        "/images/generations"
        | "/v1/images/generations"
        | "/v1/v1/images/generations"
        | "/codex/v1/images/generations" => Some(ImageProxyOperation::Generate),
        "/images/edits" | "/v1/images/edits" | "/v1/v1/images/edits" | "/codex/v1/images/edits" => {
            Some(ImageProxyOperation::Edit)
        }
        _ => None,
    }
}

pub async fn open_official_images_proxy_request(
    body: &str,
    operation: ImageProxyOperation,
    original_user_agent: Option<&str>,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_images_proxy_request_for_path(body, operation, original_user_agent, "/v1/images").await
}

pub async fn open_images_proxy_request_for_path(
    body: &str,
    operation: ImageProxyOperation,
    original_user_agent: Option<&str>,
    request_path: &str,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_images_proxy_request_for_path_with_session(
        body,
        operation,
        original_user_agent,
        request_path,
        ProxySessionHeaders::default(),
    )
    .await
}

pub async fn open_images_proxy_request_for_path_with_session(
    body: &str,
    operation: ImageProxyOperation,
    original_user_agent: Option<&str>,
    request_path: &str,
    session_headers: ProxySessionHeaders<'_>,
) -> anyhow::Result<UpstreamProxyResponse> {
    let settings = SettingsStore::default().load().unwrap_or_default();
    let provider_id = crate::official_experience::route_provider_id_from_path(request_path);
    if let Some(provider_id) = provider_id.as_deref() {
        if settings.official_experience.follows_current_account()
            && crate::official_child_routing::official_provider(provider_id)
        {
            return open_following_official_images_request(
                body,
                settings,
                operation,
                operation.official_endpoint(),
                original_user_agent,
                provider_id,
                session_headers,
            )
            .await;
        }
        if provider_id == crate::official_experience::PRIMARY_OFFICIAL_PROVIDER_ID {
            anyhow::bail!("主官方图片请求必须使用官方客户端原生链路")
        }
        if provider_id != crate::official_experience::LEGACY_CUSTOM_PROVIDER_ID {
            return open_source_images_proxy_request(
                body,
                &settings,
                operation,
                original_user_agent,
                provider_id,
            )
            .await;
        }
    }
    open_official_images_proxy_request_with_settings_and_endpoint_and_user_agent(
        body,
        settings,
        operation,
        operation.official_endpoint(),
        original_user_agent,
        None,
    )
    .await
}

async fn open_following_official_images_request(
    body: &str,
    settings: crate::settings::BackendSettings,
    operation: ImageProxyOperation,
    endpoint: &str,
    original_user_agent: Option<&str>,
    provider_id: &str,
    session_headers: ProxySessionHeaders<'_>,
) -> anyhow::Result<UpstreamProxyResponse> {
    let inherited = session_headers
        .following_provider(&settings, provider_id)
        .await?;
    let provider_id = inherited.as_deref().unwrap_or(provider_id);
    let request: Value = serde_json::from_str(body)?;
    anyhow::ensure!(
        request.get("model").and_then(Value::as_str) == Some("gpt-image-2"),
        "官方内置图片跟随只接受 gpt-image-2"
    );
    let account_id = if provider_id == "openai" {
        settings.active_official_account_id.clone()
    } else {
        crate::official_accounts::OfficialAccountStore::default()
            .list()?
            .into_iter()
            .find(|account| {
                crate::official_experience::direct_official_provider_id(&account.id) == provider_id
            })
            .context("图片来源不是固定官方账号；调度图片请求不自动换号")?
            .id
    };
    let auth = resolve_stored_official_account_auth(&account_id).await?;
    let response = open_official_images_proxy_request_with_settings_and_endpoint_and_user_agent(
        body,
        settings,
        operation,
        endpoint,
        original_user_agent,
        Some(auth),
    )
    .await?;
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "protocol_proxy.child_official_image_dispatch",
        json!({
            "sourceId": account_id, "providerId": provider_id, "operation": operation.name(),
            "threadId": session_headers.thread_id, "sessionId": session_headers.session_id,
            "phase": "response_headers", "statusCode": response.status_code, "willFailover": false,
        }),
    );
    Ok(response)
}

async fn open_source_images_proxy_request(
    body: &str,
    settings: &crate::settings::BackendSettings,
    operation: ImageProxyOperation,
    original_user_agent: Option<&str>,
    provider_id: &str,
) -> anyhow::Result<UpstreamProxyResponse> {
    if !settings.official_experience.enabled {
        anyhow::bail!("扩展图片来源未启用；请先启动 Codex++")
    }
    let request_json: Value = serde_json::from_str(body).context("图片请求必须是 JSON")?;
    let routing_slug = request_json
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .context("图片请求缺少模型，无法验证来源归属")?
        .to_string();
    let descriptors = crate::official_experience::model_route_descriptors(settings);
    let historical_model = crate::official_experience::descriptor_for_provider_and_model(
        settings,
        provider_id,
        &routing_slug,
    );
    let matching_model = descriptors
        .iter()
        .find(|descriptor| descriptor.routing_slug.eq_ignore_ascii_case(&routing_slug))
        .or(historical_model.as_ref());
    if let Some(descriptor) = matching_model
        && descriptor.source_kind != crate::settings::OfficialExperienceSourceKind::MainOfficial
        && !descriptor.provider_id.eq_ignore_ascii_case(provider_id)
    {
        anyhow::bail!(
            "图片模型「{}」不属于来源 provider「{}」，已关闭式拒绝",
            routing_slug,
            provider_id
        )
    }
    let descriptor = matching_model
        .or_else(|| {
            descriptors
                .iter()
                .find(|descriptor| descriptor.provider_id == provider_id)
        })
        .with_context(|| format!("图片来源 provider「{}」不可用", provider_id))?;
    if descriptor.source_kind == crate::settings::OfficialExperienceSourceKind::DirectOfficial {
        if !descriptor.routing_slug.eq_ignore_ascii_case(&routing_slug) {
            anyhow::bail!("图片模型「{}」不属于所选官方账号", routing_slug)
        }
        let mut upstream_json = request_json;
        upstream_json["model"] = Value::String(descriptor.upstream_model.clone());
        let source_id = if descriptor.provider_id == crate::official_account_schedule::PROVIDER_ID {
            crate::official_account_schedule::select_account(
                settings,
                &descriptor.upstream_model,
                &std::collections::HashSet::new(),
            )?
            .id
        } else {
            descriptor.source_id.clone()
        };
        let auth = resolve_stored_official_account_auth(&source_id).await?;
        let client =
            crate::http_client::proxied_client(&effective_user_agent("", original_user_agent))?;
        let upstream = send_upstream_request_with_header_timeout(
            client
                .post(operation.official_endpoint())
                .bearer_auth(&auth.access_token)
                .header("ChatGPT-Account-Id", &auth.account_id)
                .header("originator", "codex_cli_rs")
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .json(&upstream_json),
            UPSTREAM_IMAGE_HEADER_TIMEOUT,
        )
        .await
        .context("所选官方账号图片请求失败；不会切换账号")?;
        let status_code = upstream.status().as_u16();
        let content_type = upstream
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/json; charset=utf-8")
            .to_string();
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "protocol_proxy.direct_official_image_response",
            json!({"sourceId": source_id, "operation": operation.name(), "statusCode": status_code}),
        );
        return Ok(UpstreamProxyResponse {
            status_code,
            is_stream: false,
            content_type,
            wire_api: match operation {
                ImageProxyOperation::Generate => UpstreamWireApi::ImageGenerations,
                ImageProxyOperation::Edit => UpstreamWireApi::ImageEdits,
            },
            capacity_retryable: false,
            capacity_retry_enabled: false,
            capacity_retry_key: None,
            capacity_retry_max_attempts: 0,
            prefetched_chunk: Vec::new(),
            compaction: false,
            response: upstream,
            scheduled_account_id: None,
            native_compaction_passthrough: false,
            request_trace: None,
            exec_adapted: false,
            exec_outcome: None,
            _channel_permit: None,
        });
    }
    let relay = crate::official_experience::source_profile_for_descriptor(settings, &descriptor)
        .with_context(|| format!("图片来源不存在或不支持图片端点：{}", descriptor.source_id))?;
    let endpoint = match descriptor.source_kind {
        crate::settings::OfficialExperienceSourceKind::CliOfficial
        | crate::settings::OfficialExperienceSourceKind::CliGeneral => {
            let base_url = crate::relay_config::relay_profile_base_url(relay);
            if base_url.trim().is_empty() {
                anyhow::bail!("该来源不支持图片：CLIProxyAPI 未配置 API Base URL")
            }
            format!(
                "{}/images/{}",
                base_url.trim().trim_end_matches('/'),
                match operation {
                    ImageProxyOperation::Generate => "generations",
                    ImageProxyOperation::Edit => "edits",
                }
            )
        }
        crate::settings::OfficialExperienceSourceKind::Relay => {
            let endpoints = settings
                .official_experience
                .source_image_endpoints
                .get(&descriptor.source_id)
                .context("该来源不支持图片：未配置独立图片端点")?;
            let endpoint = match operation {
                ImageProxyOperation::Generate => endpoints.generations_url.trim(),
                ImageProxyOperation::Edit => endpoints.edits_url.trim(),
            };
            if endpoint.is_empty() {
                anyhow::bail!("该来源不支持图片：当前操作没有配置端点")
            }
            endpoint.to_string()
        }
        _ => anyhow::bail!("该来源不支持图片"),
    };
    let mut upstream_json = request_json;
    if descriptor.routing_slug.eq_ignore_ascii_case(&routing_slug) {
        upstream_json["model"] = Value::String(descriptor.upstream_model.clone());
    }
    let client = crate::http_client::proxied_client(&effective_user_agent(
        &relay.user_agent,
        original_user_agent,
    ))?;
    let mut request = client
        .post(&endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&upstream_json);
    if !relay.uses_no_auth() {
        let api_key = crate::relay_config::relay_profile_api_key(relay);
        if api_key.trim().is_empty() {
            anyhow::bail!("该来源不支持图片：未配置来源认证")
        }
        request = request.bearer_auth(api_key);
    }
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "protocol_proxy.source_image_request",
        json!({
            "providerId": provider_id,
            "sourceKind": descriptor.source_kind,
            "sourceId": descriptor.source_id,
            "operation": operation.name(),
            "willFailover": false
        }),
    );
    let upstream =
        send_upstream_request_with_header_timeout(request, UPSTREAM_IMAGE_HEADER_TIMEOUT)
            .await
            .with_context(|| {
                format!(
                    "来源「{}」图片请求失败；已禁止借用其他账号或供应商",
                    descriptor.source_id
                )
            })?;
    let status_code = upstream.status().as_u16();
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json; charset=utf-8")
        .to_string();
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "protocol_proxy.source_image_response",
        json!({
            "providerId": provider_id,
            "sourceKind": descriptor.source_kind,
            "sourceId": descriptor.source_id,
            "operation": operation.name(),
            "statusCode": status_code,
            "willFailover": false
        }),
    );
    Ok(UpstreamProxyResponse {
        status_code,
        is_stream: false,
        content_type,
        wire_api: match operation {
            ImageProxyOperation::Generate => UpstreamWireApi::ImageGenerations,
            ImageProxyOperation::Edit => UpstreamWireApi::ImageEdits,
        },
        capacity_retryable: false,
        capacity_retry_enabled: false,
        capacity_retry_key: None,
        capacity_retry_max_attempts: 0,
        prefetched_chunk: Vec::new(),
        compaction: false,
        response: upstream,
        scheduled_account_id: None,
        native_compaction_passthrough: false,
        request_trace: None,
        exec_adapted: false,
        exec_outcome: None,
        _channel_permit: None,
    })
}

#[doc(hidden)]
pub async fn open_official_images_proxy_request_with_settings_and_endpoint(
    body: &str,
    settings: crate::settings::BackendSettings,
    operation: ImageProxyOperation,
    official_endpoint: &str,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_official_images_proxy_request_with_settings_and_endpoint_and_user_agent(
        body,
        settings,
        operation,
        official_endpoint,
        None,
        None,
    )
    .await
}

async fn open_official_images_proxy_request_with_settings_and_endpoint_and_user_agent(
    body: &str,
    settings: crate::settings::BackendSettings,
    operation: ImageProxyOperation,
    official_endpoint: &str,
    original_user_agent: Option<&str>,
    auth_override: Option<OfficialChatGptAuth>,
) -> anyhow::Result<UpstreamProxyResponse> {
    if !settings.official_login_mixed_mode && auth_override.is_none() {
        anyhow::bail!("官方图像代理仅在官方登录混合模式下启用");
    }
    let request_json: Value = serde_json::from_str(body).context("官方图像请求必须是 JSON")?;
    let auth = match auth_override {
        Some(auth) => auth,
        None => resolve_official_chatgpt_auth(&settings)?,
    };
    let configured_user_agent = settings
        .official_login_relay_profile()
        .map(|profile| profile.user_agent.as_str())
        .unwrap_or("");
    let client = crate::http_client::proxied_client(&effective_user_agent(
        configured_user_agent,
        original_user_agent,
    ))?;
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "protocol_proxy.official_image_request",
        json!({
            "route": "official_chatgpt",
            "operation": operation.name(),
            "endpoint": official_endpoint,
            "candidateCount": 1,
            "willFailover": false
        }),
    );
    let upstream = send_upstream_request_with_header_timeout(
        client
            .post(official_endpoint)
            .bearer_auth(&auth.access_token)
            .header("ChatGPT-Account-Id", &auth.account_id)
            .header("originator", "codex_cli_rs")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&request_json),
        UPSTREAM_IMAGE_HEADER_TIMEOUT,
    )
    .await
    .context("官方 ChatGPT 图像请求失败；已禁止回退到第三方供应商")?;
    let status_code = upstream.status().as_u16();
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json; charset=utf-8")
        .to_string();
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "protocol_proxy.official_image_response",
        json!({
            "route": "official_chatgpt",
            "operation": operation.name(),
            "endpoint": official_endpoint,
            "statusCode": status_code,
            "candidateCount": 1,
            "willFailover": false
        }),
    );
    Ok(UpstreamProxyResponse {
        status_code,
        is_stream: false,
        content_type,
        wire_api: UpstreamWireApi::Responses,
        capacity_retryable: false,
        capacity_retry_enabled: false,
        capacity_retry_key: None,
        capacity_retry_max_attempts: 0,
        prefetched_chunk: Vec::new(),
        compaction: false,
        response: upstream,
        scheduled_account_id: None,
        native_compaction_passthrough: false,
        request_trace: None,
        exec_adapted: false,
        exec_outcome: None,
        _channel_permit: None,
    })
}

pub fn is_image_generations_proxy_path(path: &str) -> bool {
    let path = crate::official_experience::route_operation_path(path);
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    matches!(
        path,
        "/images/generations"
            | "/v1/images/generations"
            | "/v1/v1/images/generations"
            | "/codex/v1/images/generations"
    )
}

pub fn is_image_edits_proxy_path(path: &str) -> bool {
    let path = crate::official_experience::route_operation_path(path);
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    matches!(
        path,
        "/images/edits" | "/v1/images/edits" | "/v1/v1/images/edits" | "/codex/v1/images/edits"
    )
}

pub async fn open_responses_proxy_request(
    body: &str,
    original_user_agent: Option<&str>,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_for_path(body, original_user_agent, "/responses").await
}

pub async fn open_responses_proxy_request_for_path(
    body: &str,
    original_user_agent: Option<&str>,
    request_path: &str,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_for_path_with_beta(body, original_user_agent, request_path, None)
        .await
}

/// 只转发客户端声明的 beta 能力，不转发客户端鉴权。
pub async fn open_responses_proxy_request_for_path_with_beta(
    body: &str,
    original_user_agent: Option<&str>,
    request_path: &str,
    beta_features: Option<&str>,
) -> anyhow::Result<UpstreamProxyResponse> {
    let settings = SettingsStore::default().load().unwrap_or_default();
    open_responses_proxy_request_with_settings_and_user_agent(
        body,
        settings,
        original_user_agent,
        request_path,
        beta_features,
        ProxySessionHeaders::default(),
    )
    .await
}

pub async fn open_responses_proxy_request_with_settings(
    body: &str,
    settings: crate::settings::BackendSettings,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_with_settings_and_user_agent(
        body,
        settings,
        None,
        "/responses",
        None,
        ProxySessionHeaders::default(),
    )
    .await
}

pub async fn open_responses_proxy_request_with_settings_for_path(
    body: &str,
    settings: crate::settings::BackendSettings,
    request_path: &str,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_with_settings_and_user_agent(
        body,
        settings,
        None,
        request_path,
        None,
        ProxySessionHeaders::default(),
    )
    .await
}

pub async fn open_responses_proxy_request_with_settings_for_path_and_beta(
    body: &str,
    settings: crate::settings::BackendSettings,
    request_path: &str,
    beta_features: Option<&str>,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_with_settings_and_user_agent(
        body,
        settings,
        None,
        request_path,
        beta_features,
        ProxySessionHeaders::default(),
    )
    .await
}

#[doc(hidden)]
pub async fn open_responses_proxy_request_with_settings_and_official_endpoint(
    body: &str,
    settings: crate::settings::BackendSettings,
    official_endpoint: &str,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_with_settings_and_user_agent_and_official_endpoint(
        body,
        settings,
        None,
        "/responses",
        official_endpoint,
        None,
        ProxySessionHeaders::default(),
    )
    .await
}

pub(super) async fn open_responses_proxy_request_with_settings_and_user_agent(
    body: &str,
    settings: crate::settings::BackendSettings,
    original_user_agent: Option<&str>,
    request_path: &str,
    beta_features: Option<&str>,
    session_headers: ProxySessionHeaders<'_>,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_with_settings_and_user_agent_and_official_endpoint(
        body,
        settings,
        original_user_agent,
        request_path,
        OFFICIAL_CHATGPT_CODEX_RESPONSES_URL,
        beta_features,
        session_headers,
    )
    .await
}

async fn open_selected_vendor_compaction(
    settings: &crate::settings::BackendSettings,
    relay: &crate::settings::RelayProfile,
    mut request: Value,
    request_path: &str,
    model_override: Option<&str>,
    original_user_agent: Option<&str>,
    beta_features: Option<&str>,
    session_headers: ProxySessionHeaders<'_>,
    channel_permit: crate::channel_protection::ChannelPermit,
) -> anyhow::Result<UpstreamProxyResponse> {
    if let Some(model) = model_override {
        request["model"] = json!(model);
    }
    let mut vendor = relay.clone();
    // 到这里官方请求已独立返回；Official+mix 是历史第三方 API 表示。
    vendor.relay_mode = crate::settings::RelayMode::MixedApi;
    vendor.official_mix_api_key = false;
    vendor.model_routes.clear();
    vendor.model_mappings_enabled = false;
    vendor.model_mappings.clear();
    let mut fixed = settings.clone();
    fixed.official_experience.enabled = false;
    fixed.official_login_mixed_mode = false;
    fixed.active_relay_id = vendor.id.clone();
    fixed.active_aggregate_relay_id.clear();
    fixed.aggregate_relay_profiles.clear();
    fixed.relay_profiles = vec![vendor.clone()];
    let mut result = super::open_vendor_responses_request(
        &serde_json::to_string(&request)?,
        fixed,
        original_user_agent,
        request_path,
        beta_features,
        super::ProxySessionHeaders {
            session_id: session_headers.session_id,
            thread_id: session_headers.thread_id,
            opencode_session: session_headers.opencode_session,
            client_identity: &[],
        },
        None,
        Some(super::ForcedRelay {
            relay: vendor,
            upstream_model: None,
            auto_retry: true,
        }),
    )
    .await?;
    let response = if let Some(body) = result.buffered_body.take() {
        http::Response::builder()
            .status(result.status_code)
            .header(reqwest::header::CONTENT_TYPE, &result.content_type)
            .body(reqwest::Body::from(body))?
            .into()
    } else {
        result.response.take().context("供应商压缩响应缺少 body")?
    };
    let wire_api = match result.wire_api {
        super::UpstreamWireApi::Responses => UpstreamWireApi::Responses,
        super::UpstreamWireApi::ChatCompletions => UpstreamWireApi::ChatCompletions,
        super::UpstreamWireApi::AnthropicMessages => UpstreamWireApi::AnthropicMessages,
        super::UpstreamWireApi::AudioTranscriptions => UpstreamWireApi::AudioTranscriptions,
        super::UpstreamWireApi::ImageGenerations => UpstreamWireApi::ImageGenerations,
        super::UpstreamWireApi::ImageEdits => UpstreamWireApi::ImageEdits,
    };
    Ok(UpstreamProxyResponse {
        status_code: result.status_code,
        content_type: result.content_type,
        is_stream: result.is_stream,
        wire_api,
        capacity_retryable: result.capacity_retryable,
        capacity_retry_enabled: result.capacity_retry_enabled,
        capacity_retry_key: result.capacity_retry_key,
        capacity_retry_max_attempts: result.capacity_retry_max_attempts,
        prefetched_chunk: result.stream_prefix,
        compaction: result.compaction,
        native_compaction_passthrough: result.native_compaction_passthrough,
        request_trace: result.request_trace,
        exec_adapted: result.exec_adapted,
        exec_outcome: result.exec_outcome,
        response,
        scheduled_account_id: result.scheduled_account_id,
        _channel_permit: Some(channel_permit),
    })
}

async fn open_responses_proxy_request_with_settings_and_user_agent_and_official_endpoint(
    body: &str,
    mut settings: crate::settings::BackendSettings,
    original_user_agent: Option<&str>,
    request_path: &str,
    official_endpoint: &str,
    beta_features: Option<&str>,
    session_headers: ProxySessionHeaders<'_>,
) -> anyhow::Result<UpstreamProxyResponse> {
    let mut request_json: Value = serde_json::from_str(body)?;
    let codex_app_tools_filter =
        crate::codex_app_tools_compat::filter_stale_codex_app_dynamic_tools(&mut request_json);
    if codex_app_tools_filter.observed() {
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "protocol_proxy.codex_app_tools_compat",
            json!({
                "mcpSeen": codex_app_tools_filter.mcp_seen,
                "legacyDynamicCount": codex_app_tools_filter.legacy_dynamic_count,
                "filteredCount": codex_app_tools_filter.filtered_count,
                "changed": codex_app_tools_filter.changed(),
            }),
        );
    }
    let capacity_retry_key = settings
        .codex_app_capacity_retry
        .then(|| capacity_retry_request_key(&request_json));
    let is_stream = request_json
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut source_model = request_json
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let mut route_provider_id =
        crate::official_experience::route_provider_id_from_path(request_path);
    if let Some(original) = route_provider_id.as_deref()
        && let Some(inherited) = session_headers
            .following_provider(&settings, original)
            .await?
    {
        // 仅剥离旧实例自己的官方别名；不能借跟随规则接纳其他账号/供应商别名。
        if let Some((prefix, bare)) = source_model.split_once(':') {
            anyhow::ensure!(
                prefix == original,
                "旧子代理模型别名与原来源不一致，停止账号跟随"
            );
            source_model = bare.to_string();
            request_json["model"] = json!(source_model);
        }
        if inherited != original {
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "protocol_proxy.child_account_follow",
                json!({
                    "from": original, "to": inherited, "threadId": session_headers.thread_id, "sessionId": session_headers.session_id,
                }),
            );
        }
        route_provider_id = Some(inherited);
    }
    let official_endpoint_for_path = if is_responses_compact_proxy_path(request_path) {
        format!("{}/compact", official_endpoint.trim_end_matches('/'))
    } else {
        official_endpoint.to_string()
    };
    let route_descriptor = match route_provider_id.as_deref() {
        Some(crate::official_experience::PRIMARY_OFFICIAL_PROVIDER_ID) => {
            anyhow::ensure!(
                settings.official_experience.follows_current_account(),
                "主官方模型不得经过 Codex++ 本地协议代理"
            );
            let mut descriptor = crate::official_experience::descriptor_for_provider_and_model(
                &settings,
                "openai",
                &source_model,
            )
            .context("主官方模型不在可信目录；停止请求")?;
            descriptor.source_id = settings.active_official_account_id.clone();
            descriptor.source_kind = crate::settings::OfficialExperienceSourceKind::DirectOfficial;
            Some(descriptor)
        }
        Some(crate::official_experience::LEGACY_CUSTOM_PROVIDER_ID) | None => None,
        Some(provider_id) => {
            if !settings.official_experience.enabled {
                anyhow::bail!("扩展模型路由未启用；请先启动 Codex++")
            }
            let descriptor = crate::official_experience::descriptor_for_provider_and_model(
                &settings,
                provider_id,
                &source_model,
            )
            .with_context(|| {
                format!(
                    "模型「{}」不属于来源 provider「{}」，已关闭式拒绝",
                    source_model, provider_id
                )
            })?;
            if descriptor.failover_policy != "strict" {
                anyhow::bail!("扩展模型路由仅允许 strict 失败策略")
            }
            if descriptor.source_kind == crate::settings::OfficialExperienceSourceKind::Aggregate {
                let aggregate_id = descriptor.source_id.clone();
                let aggregate_relay = settings
                    .relay_profiles
                    .iter()
                    .find(|profile| {
                        profile.id == aggregate_id
                            && profile.relay_mode == crate::settings::RelayMode::Aggregate
                    })
                    .with_context(|| format!("聚合来源不存在：{aggregate_id}"))?;
                settings.active_relay_id = aggregate_relay.id.clone();
                settings.active_aggregate_relay_id = aggregate_id;
            }
            Some(descriptor)
        }
    };
    let review_route =
        crate::aggregate_catalog::resolve_auto_review_member(&settings, &source_model)?;
    let model_route = if let Some(review) = review_route {
        anyhow::ensure!(
            !crate::official_login::member_uses_official_login_transport(&review.relay),
            "聚合审批成员必须使用独立 API 来源"
        );
        Some(ModelRouteSelection {
            relay: review.relay,
            source_relay_id: review.relay_id,
            source_model: source_model.clone(),
            upstream_model: review.upstream_model,
        })
    } else if let Some(descriptor) = route_descriptor.as_ref() {
        if descriptor.source_kind == crate::settings::OfficialExperienceSourceKind::Aggregate
            || descriptor.source_kind
                == crate::settings::OfficialExperienceSourceKind::DirectOfficial
        {
            None
        } else {
            let relay =
                crate::official_experience::source_profile_for_descriptor(&settings, descriptor)
                    .cloned()
                    .with_context(|| {
                        format!("扩展模型来源不存在或类型不匹配：{}", descriptor.source_id)
                    })?;
            if relay.protocol != RelayProtocol::Responses {
                anyhow::bail!(
                    "来源「{}」未声明完整 Responses 能力，无法承载官方能力模板",
                    descriptor.source_id
                )
            }
            Some(ModelRouteSelection {
                relay,
                source_relay_id: descriptor.source_id.clone(),
                source_model: descriptor.routing_slug.clone(),
                upstream_model: descriptor.upstream_model.clone(),
            })
        }
    } else {
        select_model_route(&settings, &source_model)?
    };
    if let Some(descriptor) = route_descriptor.as_ref()
        && descriptor.upstream_model != source_model
    {
        request_json["model"] = Value::String(descriptor.upstream_model.clone());
    }
    if let Some(descriptor) = route_descriptor.as_ref()
        && descriptor.source_kind == crate::settings::OfficialExperienceSourceKind::DirectOfficial
    {
        if descriptor.provider_id == crate::official_account_schedule::PROVIDER_ID {
            return open_scheduled_official_request(
                &settings,
                request_json,
                is_stream,
                original_user_agent,
                &official_endpoint_for_path,
                session_headers,
            )
            .await;
        }
        let auth = resolve_stored_official_account_auth(&descriptor.source_id).await?;
        let account_name = crate::official_accounts::OfficialAccountStore::default()
            .get(&descriptor.source_id)?
            .name;
        crate::request_log::report_current_source(
            &descriptor.source_id,
            &account_name,
            &descriptor.upstream_model,
        );
        let mut response = open_official_chatgpt_responses_request(
            &settings,
            request_json,
            is_stream,
            original_user_agent,
            &official_endpoint_for_path,
            Some(auth),
            session_headers,
        )
        .await?;
        response.request_trace = Some(crate::request_log::RequestTrace {
            relay_id: descriptor.source_id.clone(),
            relay_name: account_name,
            upstream_model: descriptor.upstream_model.clone(),
            wire_api: "Responses".to_string(),
            attempts: 1,
            ..Default::default()
        });
        // 已验证凭据身份且收到响应头；此事件不表示流已成功完成。
        let audit_uuid = |value: Option<&str>| {
            value.and_then(|id| {
                uuid::Uuid::parse_str(id.trim())
                    .ok()
                    .map(|id| id.to_string())
            })
        };
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "protocol_proxy.direct_official_dispatch",
            json!({
                "sourceId": descriptor.source_id, "providerId": descriptor.provider_id,
                "upstreamModel": descriptor.upstream_model,
                "operation": if is_responses_compact_proxy_path(request_path) { "responses/compact" } else { "responses" },
                "threadId": audit_uuid(session_headers.thread_id), "sessionId": audit_uuid(session_headers.session_id),
                "phase": "response_headers", "statusCode": response.status_code, "willFailover": false,
            }),
        );
        return Ok(response);
    }
    if let Some(route) = &model_route
        && route.upstream_model != source_model
    {
        request_json["model"] = Value::String(route.upstream_model.clone());
    }
    let model = (!source_model.trim().is_empty()).then(|| source_model.clone());
    let context = RotationContext {
        conversation_id: conversation_id_from_responses_request(&request_json),
        model,
    };
    let requested_model = request_json
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty());
    let (relay, relays, track_aggregate_rotation) = if let Some(route) = &model_route {
        (route.relay.clone(), vec![route.relay.clone()], false)
    } else if route_descriptor.as_ref().is_some_and(|descriptor| {
        descriptor.source_kind == crate::settings::OfficialExperienceSourceKind::Aggregate
    }) {
        let relay =
            crate::relay_rotation::select_relay_for_request(&settings, context, requested_model)?;
        (relay.clone(), vec![relay], false)
    } else {
        let route = crate::relay_rotation::classify_mixed_model_route(&settings, requested_model);
        let (relay, track_aggregate_rotation) = match route {
            crate::relay_rotation::MixedModelRoute::Official => {
                return open_official_chatgpt_responses_request(
                    &settings,
                    request_json,
                    is_stream,
                    original_user_agent,
                    &official_endpoint_for_path,
                    None,
                    session_headers,
                )
                .await;
            }
            crate::relay_rotation::MixedModelRoute::DedicatedRelay => (
                crate::relay_rotation::select_dedicated_relay_for_model(
                    &settings,
                    requested_model,
                )?,
                false,
            ),
            crate::relay_rotation::MixedModelRoute::Reject => {
                anyhow::bail!(
                    "官方登录混合模式拒绝未知模型「{}」；请选择官方原生模型、CLIProxyAPI 专用模型、聚合括号别名或供应商:模型",
                    requested_model.unwrap_or("<missing>")
                );
            }
            crate::relay_rotation::MixedModelRoute::Aggregate => (
                crate::relay_rotation::select_relay_for_request(
                    &settings,
                    context,
                    requested_model,
                )?,
                true,
            ),
        };
        let mut relays = vec![relay.clone()];
        if track_aggregate_rotation {
            relays.extend(crate::relay_rotation::fallback_relays_after(
                &settings,
                &relay.id,
                requested_model,
            )?);
        }
        (relay, relays, track_aggregate_rotation)
    };
    debug_assert_eq!(
        relays.first().map(|item| item.id.as_str()),
        Some(relay.id.as_str())
    );
    let relay_count = relays.len();
    let mut cooldown_retries = 0_usize;
    'request: loop {
        for (attempt, relay) in relays.iter().cloned().enumerate() {
            validate_upstream(&relay)?;
            let channel_key = crate::channel_protection::key_for_relay(&relay);
            let channel_permit = crate::channel_protection::acquire(&channel_key, &relay).await;
            let model_override = model_route
                .as_ref()
                .map(|route| route.upstream_model.clone())
                .or_else(|| aggregate_upstream_model_override(&settings, &relay, requested_model))
                .or_else(|| {
                    super::auto_review_model_rewrite(&relay, &source_model).map(str::to_string)
                });
            if is_responses_compact_proxy_path(request_path)
                || request_has_compaction_trigger(&request_json)
                || super::input_has_native_compaction(&request_json)
            {
                // 官方直连已经在此前返回。这里只复用已选定供应商的协议能力，
                // 不向内部执行器传递成员列表或来源路由，不能改变请求归属。
                return open_selected_vendor_compaction(
                    &settings,
                    &relay,
                    request_json.clone(),
                    request_path,
                    model_override.as_deref(),
                    original_user_agent,
                    beta_features,
                    session_headers,
                    channel_permit,
                )
                .await;
            }
            let (endpoint, upstream_body, wire_api, compaction) = upstream_request_parts(
                &relay,
                request_json.clone(),
                request_path,
                model_override.as_deref(),
            )
            .await?;
            let is_compaction_request = compaction;
            let has_more_candidates = attempt + 1 < relay_count;
            let header_timeout = response_header_timeout(is_stream);
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "protocol_proxy.upstream_request",
                json!({
                    "relayId": relay.id,
                    "relayName": relay.name,
                    "endpoint": endpoint,
                    "wireApi": wire_api,
                    "stream": is_stream,
                    "attempt": attempt + 1,
                    "candidateCount": relay_count,
                    "headerTimeoutSeconds": header_timeout.as_secs(),
                    "modelRoute": model_route.as_ref().map(|route| json!({
                        "sourceRelayId": route.source_relay_id,
                        "sourceModel": route.source_model,
                        "targetRelayId": route.relay.id,
                        "upstreamModel": route.upstream_model
                    })),
                    "routeDescriptor": route_descriptor.as_ref().map(|descriptor| json!({
                        "providerId": descriptor.provider_id,
                        "sourceKind": descriptor.source_kind,
                        "sourceId": descriptor.source_id,
                        "routingSlug": descriptor.routing_slug,
                        "upstreamModel": descriptor.upstream_model,
                        "capabilitySlug": descriptor.capability_slug,
                        "failoverPolicy": descriptor.failover_policy
                    }))
                }),
            );
            let mut builder = upstream_request_builder(
                crate::http_client::proxied_client(&effective_user_agent(
                    &relay.user_agent,
                    original_user_agent,
                ))?,
                &endpoint,
                &relay,
                is_stream,
                &upstream_body,
            );
            builder = session_headers.apply(builder, Some(&relay));
            if wire_api == UpstreamWireApi::Responses {
                if let Some(value) = beta_features.filter(|value| !value.is_empty()) {
                    builder = builder.header("x-codex-beta-features", value);
                }
            }
            let mut upstream = match send_upstream_request_for_responses(builder, is_stream).await {
                Ok(upstream) => upstream,
                Err(error) => {
                    drop(channel_permit);
                    let _ = crate::diagnostic_log::append_diagnostic_log(
                        "protocol_proxy.upstream_request_failed",
                        json!({
                            "relayId": relay.id,
                            "relayName": relay.name,
                            "endpoint": endpoint,
                            "wireApi": wire_api,
                            "stream": is_stream,
                            "attempt": attempt + 1,
                            "candidateCount": relay_count,
                            "headerTimeoutSeconds": header_timeout.as_secs(),
                            "willFailover": has_more_candidates,
                            "error": error.to_string()
                        }),
                    );
                    if track_aggregate_rotation {
                        crate::relay_rotation::record_relay_request_failure(&settings);
                    }
                    if has_more_candidates {
                        continue;
                    }
                    return Err(error).with_context(|| {
                        format!(
                            "供应商「{}」请求上游失败，endpoint: {}",
                            relay.name, endpoint
                        )
                    });
                }
            };
            let status_code = upstream.status().as_u16();

            let content_type = upstream
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            let mut prefetched_chunk = Vec::new();
            if settings.codex_app_capacity_retry && (200..300).contains(&status_code) && is_stream {
                match probe_stream_start_for_capacity_error(&mut upstream).await {
                    Ok(StreamCapacityProbe::CapacityError(capacity_error_chunk)) => {
                        let capacity_retry_attempt = capacity_retry_key.and_then(|key| {
                            next_capacity_retry_attempt(
                                key,
                                settings.codex_app_capacity_retry_max_attempts,
                            )
                        });
                        if capacity_retry_attempt.is_none() {
                            let _ = crate::diagnostic_log::append_diagnostic_log(
                                "protocol_proxy.upstream_stream_capacity_passthrough",
                                json!({
                                    "relayId": relay.id,
                                    "relayName": relay.name,
                                    "endpoint": endpoint,
                                    "wireApi": wire_api,
                                    "stream": true,
                                    "statusCode": status_code,
                                    "attempt": attempt + 1,
                                    "candidateCount": relay_count,
                                    "maxAttempts": settings.codex_app_capacity_retry_max_attempts,
                                    "reason": "selected_model_at_capacity"
                                }),
                            );
                            return Ok(UpstreamProxyResponse {
                                status_code,
                                content_type,
                                is_stream: true,
                                wire_api,
                                capacity_retryable: false,
                                capacity_retry_enabled: true,
                                capacity_retry_key,
                                capacity_retry_max_attempts: settings
                                    .codex_app_capacity_retry_max_attempts,
                                prefetched_chunk: capacity_error_chunk,
                                compaction: is_compaction_request,
                                response: upstream,
                                scheduled_account_id: None,
                                native_compaction_passthrough: false,
                                request_trace: None,
                                exec_adapted: false,
                                exec_outcome: None,
                                _channel_permit: Some(channel_permit),
                            });
                        }
                        let capacity_retry_attempt = capacity_retry_attempt.unwrap_or_default();
                        let _ = crate::diagnostic_log::append_diagnostic_log(
                            "protocol_proxy.upstream_stream_capacity_rewritten",
                            json!({
                                "relayId": relay.id,
                                "relayName": relay.name,
                                "endpoint": endpoint,
                                "wireApi": wire_api,
                                "stream": true,
                                "statusCode": status_code,
                                "attempt": attempt + 1,
                                "candidateCount": relay_count,
                                "capacityRetryAttempt": capacity_retry_attempt,
                                "capacityRetryMaxAttempts": settings.codex_app_capacity_retry_max_attempts,
                                "willFailover": false,
                                "reason": "selected_model_at_capacity"
                            }),
                        );
                        if track_aggregate_rotation {
                            crate::relay_rotation::record_relay_request_event(
                                &settings,
                                RotationEvent::Failure,
                            );
                        }
                        return Ok(UpstreamProxyResponse {
                            status_code: 503,
                            content_type: "application/json; charset=utf-8".to_string(),
                            is_stream: false,
                            wire_api,
                            capacity_retryable: true,
                            capacity_retry_enabled: true,
                            capacity_retry_key,
                            capacity_retry_max_attempts: settings
                                .codex_app_capacity_retry_max_attempts,
                            prefetched_chunk: Vec::new(),
                            compaction: is_compaction_request,
                            response: upstream,
                            scheduled_account_id: None,
                            native_compaction_passthrough: false,
                            request_trace: None,
                            exec_adapted: false,
                            exec_outcome: None,
                            _channel_permit: Some(channel_permit),
                        });
                    }
                    Ok(StreamCapacityProbe::Prefetched(chunk)) => prefetched_chunk = chunk,
                    Err(error) => {
                        let _ = crate::diagnostic_log::append_diagnostic_log(
                            "protocol_proxy.upstream_stream_probe_failed",
                            json!({
                                "relayId": relay.id,
                                "relayName": relay.name,
                                "endpoint": endpoint,
                                "wireApi": wire_api,
                                "stream": true,
                                "attempt": attempt + 1,
                                "candidateCount": relay_count,
                                "willFailover": has_more_candidates,
                                "error": error.to_string()
                            }),
                        );
                        if track_aggregate_rotation {
                            crate::relay_rotation::record_relay_request_failure(&settings);
                        }
                        if has_more_candidates {
                            continue;
                        }
                        return Err(error).context("读取上游流首段失败");
                    }
                }
            }
            let retry_after = crate::channel_protection::retry_after_duration(upstream.headers());

            let _ = crate::diagnostic_log::append_diagnostic_log(
                "protocol_proxy.upstream_response",
                json!({
                    "relayId": relay.id,
                    "relayName": relay.name,
                    "endpoint": endpoint,
                    "wireApi": wire_api,
                    "stream": is_stream,
                    "statusCode": status_code,
                    "attempt": attempt + 1,
                    "candidateCount": relay_count,
                    "headerTimeoutSeconds": header_timeout.as_secs(),
                    "willFailover": has_more_candidates && !(200..300).contains(&status_code)
                }),
            );
            if track_aggregate_rotation {
                crate::relay_rotation::record_relay_request_event(
                    &settings,
                    if (200..300).contains(&status_code) {
                        RotationEvent::Success
                    } else {
                        RotationEvent::Failure
                    },
                );
            }
            if (200..300).contains(&status_code) || !has_more_candidates {
                if (200..300).contains(&status_code)
                    && let Some(key) = capacity_retry_key
                {
                    reset_capacity_retry_attempts(key);
                }
                if !(200..300).contains(&status_code) {
                    let cooldown_started = crate::channel_protection::mark_failure(
                        &channel_key,
                        &relay,
                        status_code,
                        retry_after,
                    )
                    .await;
                    if cooldown_started && should_retry_after_cooldown(cooldown_retries) {
                        cooldown_retries = cooldown_retries.saturating_add(1);
                        drop(channel_permit);
                        let _ = crate::diagnostic_log::append_diagnostic_log(
                            "protocol_proxy.channel_cooldown_retry",
                            json!({
                                "relayId": relay.id,
                                "relayName": relay.name,
                                "statusCode": status_code,
                                "retryAfterSeconds": retry_after.map(|value| value.as_secs()),
                                "retry": cooldown_retries
                            }),
                        );
                        continue 'request;
                    }
                }
                return Ok(UpstreamProxyResponse {
                    status_code,
                    is_stream: is_stream || content_type.contains("text/event-stream"),
                    content_type,
                    wire_api,
                    capacity_retryable: false,
                    capacity_retry_enabled: settings.codex_app_capacity_retry,
                    capacity_retry_key,
                    capacity_retry_max_attempts: settings.codex_app_capacity_retry_max_attempts,
                    prefetched_chunk,
                    compaction: is_compaction_request,

                    response: upstream,
                    scheduled_account_id: None,
                    native_compaction_passthrough: false,
                    request_trace: Some(crate::request_log::RequestTrace {
                        relay_id: relay.id.clone(),
                        relay_name: relay.name.clone(),
                        upstream_model: upstream_body
                            .get("model")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        upstream_model_bound: model_override.is_some(),
                        wire_api: format!("{wire_api:?}"),
                        attempts: (attempt + 1) as u32,
                        ..Default::default()
                    }),
                    exec_adapted: false,
                    exec_outcome: None,
                    _channel_permit: Some(channel_permit),
                });
            }
            crate::channel_protection::mark_failure(&channel_key, &relay, status_code, retry_after)
                .await;
            drop(channel_permit);
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "protocol_proxy.upstream_failover",
                json!({
                    "relayId": relay.id,
                    "relayName": relay.name,
                    "endpoint": endpoint,
                    "wireApi": wire_api,
                    "stream": is_stream,
                    "statusCode": status_code,
                    "attempt": attempt + 1,
                    "candidateCount": relay_count,
                    "headerTimeoutSeconds": header_timeout.as_secs()
                }),
            );
        }
        anyhow::bail!("未找到可用的聚合供应商成员")
    }
}

fn select_model_route(
    settings: &crate::settings::BackendSettings,
    model: &str,
) -> anyhow::Result<Option<ModelRouteSelection>> {
    if model.is_empty() || settings.active_aggregate_relay_profile().is_some() {
        return Ok(None);
    }
    if settings.active_relay_uses_official_login_auth()
        && crate::aggregate_model_alias::is_trusted_official_codex_model_for_settings(
            model, settings,
        )
    {
        return Ok(None);
    }

    let source = settings.active_relay_profile();
    let Some(route) = source
        .model_routes
        .iter()
        .find(|route| route.model.trim() == model)
    else {
        return Ok(None);
    };
    let target_relay_id = route.target_relay_id.trim();
    if target_relay_id == source.id {
        anyhow::bail!("模型路由不能指向当前供应商自身：{model}");
    }
    let target = settings
        .relay_profiles
        .iter()
        .find(|profile| profile.id == target_relay_id)
        .cloned()
        .with_context(|| format!("模型路由目标供应商不存在：{target_relay_id}"))?;
    if target.relay_mode == crate::settings::RelayMode::Aggregate {
        anyhow::bail!("模型路由目标不能是聚合供应商：{}", target.name);
    }
    if target.protocol != RelayProtocol::Responses {
        anyhow::bail!("模型路由目标必须使用 Responses API：{}", target.name);
    }

    let upstream_model = if route.target_model.trim().is_empty() {
        model.to_string()
    } else {
        route.target_model.trim().to_string()
    };
    Ok(Some(ModelRouteSelection {
        relay: target,
        source_relay_id: source.id,
        source_model: model.to_string(),
        upstream_model,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OfficialChatGptAuth {
    access_token: String,
    account_id: String,
}

async fn resolve_stored_official_account_auth(
    account_id: &str,
) -> anyhow::Result<OfficialChatGptAuth> {
    let store = crate::official_accounts::OfficialAccountStore::default();
    resolve_stored_official_account_auth_from_store(&store, account_id).await
}

async fn resolve_stored_official_account_auth_from_store(
    store: &crate::official_accounts::OfficialAccountStore,
    account_id: &str,
) -> anyhow::Result<OfficialChatGptAuth> {
    let account = store.get(account_id)?;
    anyhow::ensure!(
        account.enabled && account.status == "ready",
        "所选官方账号不可用"
    );
    store.refresh_tokens(account_id, false).await?;
    let auth_json = store.get_auth_json(account_id)?;
    let parsed = crate::official_accounts::parse_official_auth(&auth_json)?;
    anyhow::ensure!(parsed.id == account_id, "官方账号凭据身份不匹配");
    let account_id = if parsed.chatgpt_account_id.is_empty() {
        parsed.workspace_id
    } else {
        parsed.chatgpt_account_id
    };
    let access_token = crate::official_accounts::auth_token(&auth_json, "access_token")
        .context("所选官方账号缺少访问令牌")?
        .to_string();
    Ok(OfficialChatGptAuth {
        access_token,
        account_id,
    })
}

#[cfg(test)]
mod direct_official_auth_tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn protected_official_usage_survives_every_sse_chunk_boundary() {
        let bytes = b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1200,\"output_tokens\":100,\"input_tokens_details\":{\"cached_tokens\":1100}}}}\n\n";
        for split in 0..=bytes.len() {
            let mut tracker = ResponsesSseTerminalTracker::default();
            tracker.observe(&bytes[..split]);
            tracker.observe(&bytes[split..]);
            tracker.finish();
            assert!(tracker.is_terminal() && !tracker.has_failed());
            let usage = tracker.usage.unwrap();
            assert_eq!(usage.input_tokens, 1200);
            assert_eq!(usage.output_tokens, 100);
            assert_eq!(usage.cached_tokens, 1100);
        }
    }

    fn jwt(value: Value) -> String {
        let encoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!(
            "{}.{}.sig",
            encoder.encode(br#"{"alg":"none"}"#),
            encoder.encode(serde_json::to_vec(&value).unwrap())
        )
    }

    fn auth(subject: &str, account_id: &str) -> Value {
        json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "id_token": jwt(json!({"sub": subject})),
                "access_token": jwt(json!({
                    "sub": subject,
                    "exp": 4_102_444_800_i64,
                    "https://api.openai.com/auth": {"chatgpt_account_id": account_id}
                })),
                "account_id": account_id,
                "refresh_token": "test-refresh"
            }
        })
    }

    #[tokio::test]
    async fn direct_official_dispatch_preserves_selected_account_and_session_headers() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let _guard = crate::paths::settings_path_test_guard();
        let temp = tempfile::tempdir().unwrap();
        struct Restore {
            accounts: Option<(std::path::PathBuf, std::path::PathBuf)>,
            catalog: Option<std::path::PathBuf>,
            settings: Option<std::path::PathBuf>,
            home: Option<std::ffi::OsString>,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                crate::paths::set_official_account_paths_for_tests(self.accounts.take());
                crate::paths::set_official_model_catalog_path_for_tests(self.catalog.take());
                crate::paths::set_settings_path_for_tests(self.settings.take());
                crate::diagnostic_log::set_diagnostic_log_path_for_tests(None);
                unsafe {
                    match self.home.take() {
                        Some(home) => std::env::set_var("CODEX_HOME", home),
                        None => std::env::remove_var("CODEX_HOME"),
                    }
                }
            }
        }
        let paths = (
            temp.path().join("accounts.json"),
            temp.path().join("secrets.json"),
        );
        assert!(paths.0.starts_with(temp.path()) && paths.1.starts_with(temp.path()));
        let catalog_path = temp.path().join("catalog.json");
        let _restore = Restore {
            accounts: crate::paths::set_official_account_paths_for_tests(Some(paths.clone())),
            catalog: crate::paths::set_official_model_catalog_path_for_tests(Some(
                catalog_path.clone(),
            )),
            settings: crate::paths::set_settings_path_for_tests(Some(
                temp.path().join("settings.json"),
            )),
            home: std::env::var_os("CODEX_HOME"),
        };
        unsafe {
            std::env::set_var("CODEX_HOME", temp.path());
        }
        let log_path = temp.path().join("audit.jsonl");
        crate::diagnostic_log::set_diagnostic_log_path_for_tests(Some(log_path.clone()));
        let store = crate::official_accounts::OfficialAccountStore::default();
        let first_auth = auth("first", "chatgpt-1");
        let second_auth = auth("second", "chatgpt-2");
        let (first, _) = store.upsert_auth_json(first_auth.clone()).unwrap();
        let (second, _) = store.upsert_auth_json(second_auth.clone()).unwrap();
        let mut settings = crate::settings::BackendSettings::default();
        settings.official_experience.enabled = true;
        settings.active_official_account_id = first.id.clone();
        settings.official_experience.primary_official_account_id = first.id.clone();
        std::fs::write(&catalog_path, serde_json::to_vec(&json!({"version":1,"accounts":{
            &first.id:{"accountId":first.id,"fetchedAt":1,"source":"account_snapshot","clientVersion":"0.166.0","models":[{"slug":"gpt-6.1-sol","visibility":"list"}]},
            &second.id:{"accountId":second.id,"fetchedAt":1,"source":"account_snapshot","clientVersion":"0.166.0","models":[{"slug":"gpt-6.1-sol","visibility":"list"}]}
        }})).unwrap()).unwrap();
        std::fs::write(
            temp.path().join("models_cache.json"),
            json!({"client_version":"0.157.1"}).to_string(),
        )
        .unwrap();
        let thread = "01a11443-b035-7e73-bb3e-be5e9ecd30a7";
        let session = "01a11ace-65f5-7913-a794-56db911459f9";
        let headers = ProxySessionHeaders {
            thread_id: Some(thread),
            session_id: Some(session),
            opencode_session: Some("opencode-123"),
        };
        let graph = rusqlite::Connection::open(temp.path().join("state_5.sqlite")).unwrap();
        graph.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,source TEXT,model_provider TEXT); CREATE TABLE thread_spawn_edges(parent_thread_id TEXT,child_thread_id TEXT);").unwrap();
        graph
            .execute("INSERT INTO threads VALUES(?1,'cli','openai')", [session])
            .unwrap();
        graph
            .execute(
                "INSERT INTO threads VALUES(?1,'subagent','openai')",
                [thread],
            )
            .unwrap();
        graph
            .execute(
                "INSERT INTO thread_spawn_edges VALUES(?1,?2)",
                [session, thread],
            )
            .unwrap();
        let first_provider = crate::official_experience::direct_official_provider_id(&first.id);
        let second_provider = crate::official_experience::direct_official_provider_id(&second.id);
        for (selected, selected_auth, live_auth, operation, incoming, follow) in [
            (
                &first,
                &first_auth,
                &second_auth,
                "responses",
                first_provider.as_str(),
                false,
            ),
            (
                &second,
                &second_auth,
                &first_auth,
                "responses/compact",
                second_provider.as_str(),
                false,
            ),
            (
                &second,
                &second_auth,
                &first_auth,
                "responses",
                "openai",
                false,
            ),
            (
                &second,
                &second_auth,
                &first_auth,
                "responses/compact",
                first_provider.as_str(),
                false,
            ),
            (
                &first,
                &first_auth,
                &first_auth,
                "responses",
                second_provider.as_str(),
                true,
            ),
            (
                &second,
                &second_auth,
                &first_auth,
                "images/generations",
                "openai",
                false,
            ),
            (
                &second,
                &second_auth,
                &first_auth,
                "images/edits",
                first_provider.as_str(),
                false,
            ),
        ] {
            settings.official_experience.follow_parent_account = follow;
            let provider = if selected.id == first.id {
                "openai".to_string()
            } else {
                crate::official_experience::direct_official_provider_id(&selected.id)
            };
            graph
                .execute(
                    "UPDATE threads SET model_provider=?1 WHERE id=?2",
                    [&provider, session],
                )
                .unwrap();
            std::fs::write(
                temp.path().join("auth.json"),
                serde_json::to_vec(live_auth).unwrap(),
            )
            .unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/responses", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut bytes = [0_u8; 4096];
                    let count = stream.read(&mut bytes).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&bytes[..count]);
                    if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                        let length: usize = head
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await.unwrap();
                String::from_utf8(request).unwrap()
            });
            let path = format!("/v1/routes/{incoming}/{operation}");
            let model = if incoming != "openai" {
                format!("{incoming}:gpt-6.1-sol")
            } else {
                "gpt-6.1-sol".to_string()
            };
            let body = json!({"model":model,"input":[],"stream":false}).to_string();
            let image = operation.starts_with("images/");
            let result = if image {
                open_following_official_images_request(
                    r#"{"model":"gpt-image-2","prompt":"local fixture"}"#,
                    settings.clone(),
                    if operation.ends_with("edits") {
                        ImageProxyOperation::Edit
                    } else {
                        ImageProxyOperation::Generate
                    },
                    &endpoint,
                    None,
                    incoming,
                    headers,
                )
                .await
                .unwrap()
            } else {
                open_responses_proxy_request_with_settings_and_user_agent_and_official_endpoint(
                    &body,
                    settings.clone(),
                    None,
                    &path,
                    &endpoint,
                    None,
                    headers,
                )
                .await
                .unwrap()
            };
            assert_eq!(result.status_code, 200);
            let request = server.await.unwrap();
            let expected_token = selected_auth["tokens"]["access_token"].as_str().unwrap();
            assert!(request.contains(&format!("authorization: Bearer {expected_token}")));
            assert!(
                request.contains(&format!(
                    "ChatGPT-Account-Id: {}",
                    selected_auth["tokens"]["account_id"].as_str().unwrap()
                )) || request.contains(&format!(
                    "chatgpt-account-id: {}",
                    selected_auth["tokens"]["account_id"].as_str().unwrap()
                ))
            );
            if !image {
                assert!(
                    request.contains(&format!("session-id: {session}"))
                        && request.contains(&format!("thread-id: {thread}"))
                        && request.contains("x-opencode-session: opencode-123")
                );
                assert!(request.contains(if operation.ends_with("compact") {
                    "POST /responses/compact "
                } else {
                    "POST /responses "
                }));
                let events = std::fs::read_to_string(&log_path).unwrap();
                let dispatch: Value = events
                    .lines()
                    .map(|line| serde_json::from_str::<Value>(line).unwrap())
                    .filter(|event| event["event"] == "protocol_proxy.direct_official_dispatch")
                    .last()
                    .unwrap();
                assert_eq!(dispatch["detail"]["sourceId"], selected.id);
                assert_eq!(dispatch["detail"]["providerId"], provider);
                assert_eq!(dispatch["detail"]["threadId"], thread);
                assert_eq!(dispatch["detail"]["upstreamModel"], "gpt-6.1-sol");
                assert!(!events.contains(expected_token));
                assert!(request.contains("\"model\":\"gpt-6.1-sol\""));
                let expected_version = crate::gateway::chatgpt_backend::select_codex_version(
                    Some("0.166.0"),
                    crate::codex_cli_version::installed_codex_version_cached().as_deref(),
                );
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains(&format!("\r\nversion: {expected_version}\r\n"))
                );
            } else {
                assert!(request.contains("\"model\":\"gpt-image-2\""));
                let events = std::fs::read_to_string(&log_path).unwrap();
                assert!(!events.contains("local fixture") && !events.contains(expected_token));
            }
            assert_eq!(
                graph
                    .query_row(
                        "SELECT model_provider FROM threads WHERE id=?1",
                        [thread],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                "openai"
            );
        }

        settings.official_experience.follow_parent_account = false;
        settings.official_experience.account_schedule_enabled = true;
        settings.official_experience.account_schedule_order =
            vec![second.id.clone(), first.id.clone()];
        let schedule_provider = crate::official_account_schedule::PROVIDER_ID;
        // 启动登录账号1、首选推理账号2：写目录和 provider 不能修改 live auth。
        std::fs::write(
            temp.path().join("auth.json"),
            serde_json::to_vec(&first_auth).unwrap(),
        )
        .unwrap();
        let auth_before_schedule = std::fs::read(temp.path().join("auth.json")).unwrap();
        crate::relay_config::ensure_official_experience_config_in_home(temp.path(), &settings)
            .unwrap();
        let config: toml::Table = std::fs::read_to_string(temp.path().join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(config["model_provider"].as_str(), Some("openai"));
        assert!(
            config["model_providers"][schedule_provider]["base_url"]
                .as_str()
                .unwrap()
                .ends_with(schedule_provider)
        );
        assert_eq!(
            std::fs::read(temp.path().join("auth.json")).unwrap(),
            auth_before_schedule
        );
        assert_eq!(settings.active_official_account_id, first.id);
        assert_eq!(
            settings.official_experience.primary_official_account_id,
            first.id
        );
        // Parent and child requests retain the same provider even when the wire model is bare.
        for model in [
            "gpt-6.1-sol".to_string(),
            format!("{schedule_provider}:gpt-6.1-sol"),
        ] {
            assert!(
                crate::official_experience::descriptor_for_provider_and_model(
                    &settings,
                    schedule_provider,
                    &model
                )
                .is_some()
            );
        }
        for (case, (status, payload, expected_requests)) in [
            (429, r#"{"error":{"type":"usage_limit_reached"}}"#, 2),
            (429, r#"{"error":{"code":"rate_limit_exceeded"}}"#, 1),
            (401, r#"{"error":{"code":"invalid_api_key"}}"#, 1),
            (500, r#"{"error":{"code":"server_error"}}"#, 1),
            (
                200,
                "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_limit_reached\"}}}\n\n",
                2,
            ),
            (
                200,
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\ndata: {\"error\":{\"type\":\"usage_limit_reached\"}}\n\n",
                1,
            ),
        ].into_iter().enumerate() {
            // 复用切换前的 openai 子线程，父对话改为顺序调度；出站仍须选账号2。
            settings.official_experience.follow_parent_account = case % 2 == 0;
            graph.execute("UPDATE threads SET model_provider=?1 WHERE id=?2", [schedule_provider, session]).unwrap();
            let incoming = if settings.official_experience.follow_parent_account { "openai" } else { schedule_provider };
            crate::official_account_schedule::mark_exhausted(&second.id, 0);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/responses", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let mut requests = Vec::new();
                for index in 0..expected_requests {
                    let (mut stream, _) =
                        tokio::time::timeout(Duration::from_secs(5), listener.accept())
                            .await
                            .unwrap()
                            .unwrap();
                    let mut request = Vec::new();
                    loop {
                        let mut buf = [0u8; 4096];
                        let size = stream.read(&mut buf).await.unwrap();
                        assert!(size > 0);
                        request.extend_from_slice(&buf[..size]);
                        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head =
                                String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                            let length: usize = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length: "))
                                .unwrap()
                                .parse()
                                .unwrap();
                            if request.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    requests.push(String::from_utf8(request).unwrap());
                    let (status, body) = if index == 0 {
                        (status, payload)
                    } else {
                        (200, "data: {\"type\":\"response.completed\"}\n\n")
                    };
                    let content_type = if status == 200 {
                        "text/event-stream"
                    } else {
                        "application/json"
                    };
                    let reply = format!(
                        "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(reply.as_bytes()).await.unwrap();
                }
                assert!(
                    tokio::time::timeout(Duration::from_millis(50), listener.accept())
                        .await
                        .is_err()
                );
                requests
            });
            let result =
                open_responses_proxy_request_with_settings_and_user_agent_and_official_endpoint(
                    r#"{"model":"gpt-6.1-sol","input":[],"stream":true}"#,
                    settings.clone(),
                    None,
                    &format!("/v1/routes/{incoming}/responses"),
                    &endpoint,
                    None,
                    headers,
                )
                .await
                .unwrap();
            let requests = server.await.unwrap();
            assert_eq!(requests.len(), expected_requests);
            for (index, request) in requests.iter().enumerate() {
                let selected_auth = if index == 0 {
                    &second_auth
                } else {
                    &first_auth
                };
                let token = selected_auth["tokens"]["access_token"].as_str().unwrap();
                assert!(request.contains(&format!("authorization: Bearer {token}")));
                assert!(request.to_ascii_lowercase().contains(&format!(
                    "chatgpt-account-id: {}",
                    selected_auth["tokens"]["account_id"].as_str().unwrap()
                )));
                assert!(request.contains(&format!("thread-id: {thread}")));
            }
            assert_eq!(
                result.scheduled_account_id.as_deref(),
                Some(if expected_requests == 2 {
                    first.id.as_str()
                } else {
                    second.id.as_str()
                })
            );
        }
        settings.official_experience.follow_parent_account = false;
        // A known exhausted 5h window skips account2 before any network call.
        assert_eq!(
            std::fs::read(temp.path().join("auth.json")).unwrap(),
            auth_before_schedule,
            "调度换号不能改写启动登录身份"
        );
        let usage = crate::official_accounts::parse_usage_snapshot(
            &json!({"rate_limit":{"primary_window":{
                "used_percent":100,"limit_window_seconds":18000,"reset_at":chrono::Utc::now().timestamp()+300
            }}}),
        );
        store.update_usage(&second.id, usage).unwrap();
        assert_eq!(
            crate::official_account_schedule::select_account(
                &settings,
                "gpt-6.1-sol",
                &std::collections::HashSet::new()
            )
            .unwrap()
            .id,
            first.id
        );
        let mut disabled = settings.clone();
        disabled.official_experience.account_schedule_enabled = false;
        assert!(
            crate::official_account_schedule::select_account(
                &disabled,
                "gpt-6.1-sol",
                &std::collections::HashSet::new()
            )
            .is_err()
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/responses", listener.local_addr().unwrap());
        let before = std::fs::read_to_string(&log_path).unwrap();
        for (provider, model) in [
            ("codex_plus_official_missing".to_string(), "gpt-6.1-sol"),
            (
                crate::official_experience::direct_official_provider_id(&second.id),
                "gpt-unknown",
            ),
        ] {
            assert!(
                open_responses_proxy_request_with_settings_and_user_agent_and_official_endpoint(
                    &json!({"model":model}).to_string(),
                    settings.clone(),
                    None,
                    &format!("/v1/routes/{provider}/responses"),
                    &endpoint,
                    None,
                    headers
                )
                .await
                .is_err()
            );
        }
        let image_error = open_following_official_images_request(
            r#"{"model":"gpt-image-2","prompt":"local rejected fixture"}"#,
            settings.clone(),
            ImageProxyOperation::Generate,
            &endpoint,
            None,
            "codex_plus_official_missing",
            headers,
        )
        .await
        .err()
        .expect("unknown image account must be rejected before connecting");
        assert!(image_error.to_string().contains("不存在，停止账号跟随"));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
        let after = std::fs::read_to_string(&log_path).unwrap();
        assert!(after.starts_with(&before));
        // 已知旧来源的无效模型可记录只读跟随判断，不能记为已发出官方请求。
        for event in after[before.len()..].lines() {
            let event: Value = serde_json::from_str(event).unwrap();
            assert_eq!(event["event"], "protocol_proxy.child_account_follow");
        }
    }

    #[test]
    fn client_session_header_values_are_bounded_and_cannot_inject_headers() {
        let client = reqwest::Client::new();
        let oversized = "x".repeat(1025);
        let request = ProxySessionHeaders {
            session_id: Some("ok\r\nauthorization: stolen"),
            thread_id: Some(&oversized),
            opencode_session: Some("  safe  "),
        }
        .apply(client.get("http://localhost"), None)
        .build()
        .unwrap();
        assert!(
            !request.headers().contains_key("session-id")
                && !request.headers().contains_key("thread-id")
                && !request.headers().contains_key("authorization")
        );
        assert_eq!(request.headers()["x-opencode-session"], "safe");
    }

    #[tokio::test]
    async fn explicit_account_route_reads_only_its_own_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let store = crate::official_accounts::OfficialAccountStore::new(
            temp.path().join("accounts.json"),
            temp.path().join("secrets.json"),
        );
        let (first, _) = store.upsert_auth_json(auth("first", "chatgpt-1")).unwrap();
        let (second, _) = store.upsert_auth_json(auth("second", "chatgpt-2")).unwrap();
        let first_auth = resolve_stored_official_account_auth_from_store(&store, &first.id)
            .await
            .unwrap();
        let second_auth = resolve_stored_official_account_auth_from_store(&store, &second.id)
            .await
            .unwrap();
        assert_eq!(first_auth.account_id, "chatgpt-1");
        assert_eq!(second_auth.account_id, "chatgpt-2");
        assert_ne!(first_auth.access_token, second_auth.access_token);
        assert!(
            resolve_stored_official_account_auth_from_store(&store, "unknown")
                .await
                .is_err()
        );
    }
}

fn official_chatgpt_auth_from_contents(contents: &str) -> Option<OfficialChatGptAuth> {
    let value: Value = serde_json::from_str(contents).ok()?;
    if !value
        .get("auth_mode")
        .and_then(Value::as_str)
        .is_some_and(|mode| mode.eq_ignore_ascii_case("chatgpt"))
    {
        return None;
    }
    let tokens = value.get("tokens")?;
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(OfficialChatGptAuth {
        access_token: access_token.to_string(),
        account_id: account_id.to_string(),
    })
}

fn resolve_official_chatgpt_auth(
    settings: &crate::settings::BackendSettings,
) -> anyhow::Result<OfficialChatGptAuth> {
    let selected = settings
        .official_login_relay_profile()
        .and_then(|profile| official_chatgpt_auth_from_contents(&profile.auth_contents));
    let live =
        std::fs::read_to_string(crate::codex_home::default_codex_home_dir().join("auth.json"))
            .ok()
            .and_then(|contents| official_chatgpt_auth_from_contents(&contents));

    match (live, selected) {
        (Some(live), Some(selected)) if live.account_id != selected.account_id => Ok(selected),
        (Some(live), _) => Ok(live),
        (None, Some(selected)) => Ok(selected),
        (None, None) => anyhow::bail!(
            "官方模型请求缺少有效 ChatGPT 登录：auth.json 必须包含 access_token 和 account_id"
        ),
    }
}

/// 只在响应尚未交付客户端、且官方明确报告额度耗尽时重试下一个账号。
async fn open_scheduled_official_request(
    settings: &crate::settings::BackendSettings,
    request_json: Value,
    is_stream: bool,
    original_user_agent: Option<&str>,
    endpoint: &str,
    session_headers: ProxySessionHeaders<'_>,
) -> anyhow::Result<UpstreamProxyResponse> {
    let model = request_json
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut attempted = std::collections::HashSet::new();
    loop {
        let account =
            crate::official_account_schedule::select_account(settings, model, &attempted)?;
        attempted.insert(account.id.clone());
        // 推理等级仍须被目标账号自己的目录支持，不能静默降级或换号。
        if let Some(effort) = request_json
            .pointer("/reasoning/effort")
            .and_then(Value::as_str)
        {
            let entries = crate::official_model_catalog::cached_official_model_entries_for_account(
                &account.id,
            )
            .unwrap_or_default();
            if let Some(levels) = entries
                .iter()
                .find(|e| e.get("slug").and_then(Value::as_str) == Some(model))
                .and_then(|e| e.get("supported_reasoning_levels"))
                .and_then(Value::as_array)
            {
                anyhow::ensure!(
                    levels
                        .iter()
                        .any(|v| v.get("effort").and_then(Value::as_str) == Some(effort)),
                    "顺序调度目标账号不支持请求的推理等级：{effort}"
                );
            }
        }
        let auth = resolve_stored_official_account_auth(&account.id).await?;
        crate::request_log::report_current_source(&account.id, &account.name, model);
        let mut response = open_official_chatgpt_responses_request(
            settings,
            request_json.clone(),
            is_stream,
            original_user_agent,
            endpoint,
            Some(auth),
            session_headers,
        )
        .await?;
        let now = chrono::Utc::now().timestamp();
        response.request_trace = Some(crate::request_log::RequestTrace {
            relay_id: account.id.clone(),
            relay_name: account.name.clone(),
            upstream_model: model.to_string(),
            wire_api: "Responses".to_string(),
            attempts: attempted.len() as u32,
            ..Default::default()
        });
        // 非流式拒绝和首段 SSE 额度错误可以安全重试；首段以外的输出不缓冲、不重放。
        if response.status_code == 429 || (response.is_success() && response.is_stream) {
            let limit = if response.status_code == 429 {
                64 * 1024
            } else {
                16 * 1024
            };
            while response.prefetched_chunk.len() < limit {
                if response.is_success()
                    && (response.prefetched_chunk.windows(2).any(|w| w == b"\n\n")
                        || response
                            .prefetched_chunk
                            .windows(4)
                            .any(|w| w == b"\r\n\r\n"))
                {
                    break;
                }
                match tokio::time::timeout(
                    upstream_stream_idle_timeout(),
                    response.response.chunk(),
                )
                .await
                {
                    Ok(Ok(Some(chunk))) => response.prefetched_chunk.extend_from_slice(&chunk),
                    Ok(Ok(None)) => break,
                    Ok(Err(error)) => return Err(error).context("读取顺序调度响应失败"),
                    Err(_) => anyhow::bail!("读取顺序调度响应超时"),
                }
            }
            if let Some(until) =
                crate::official_account_schedule::quota_error_reset(&response.prefetched_chunk, now)
            {
                crate::official_account_schedule::mark_exhausted(&account.id, until);
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "protocol_proxy.official_schedule_quota_exhausted",
                    json!({"sourceId": account.id, "providerId": crate::official_account_schedule::PROVIDER_ID,
                        "resetsAt": until, "attempt": attempted.len(), "phase": "before_output"}),
                );
                continue;
            }
        }
        let audit_uuid = |value: Option<&str>| {
            value
                .and_then(|id| uuid::Uuid::parse_str(id).ok())
                .map(|id| id.to_string())
        };
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "protocol_proxy.direct_official_dispatch",
            json!({"sourceId": account.id, "providerId": crate::official_account_schedule::PROVIDER_ID,
                "upstreamModel": model, "threadId": audit_uuid(session_headers.thread_id),
                "sessionId": audit_uuid(session_headers.session_id), "phase": "response_headers",
                "statusCode": response.status_code, "attempt": attempted.len(), "willFailover": false}),
        );
        response.scheduled_account_id = Some(account.id);
        return Ok(response);
    }
}

async fn open_official_chatgpt_responses_request(
    settings: &crate::settings::BackendSettings,
    mut request_json: Value,
    is_stream: bool,
    original_user_agent: Option<&str>,
    official_endpoint: &str,
    auth_override: Option<OfficialChatGptAuth>,
    session_headers: ProxySessionHeaders<'_>,
) -> anyhow::Result<UpstreamProxyResponse> {
    normalize_responses_input_items(&mut request_json);
    normalize_responses_item_ids(&mut request_json);
    let capacity_retry_key = settings
        .codex_app_capacity_retry
        .then(|| capacity_retry_request_key(&request_json));
    let auth = match auth_override {
        Some(auth) => auth,
        None => resolve_official_chatgpt_auth(settings)?,
    };
    let configured_user_agent = settings
        .official_login_relay_profile()
        .map(|profile| profile.user_agent.as_str())
        .unwrap_or("");
    let client_version = crate::gateway::chatgpt_backend::codex_version(
        &crate::codex_home::default_codex_home_dir(),
    );
    let client = crate::http_client::proxied_client(&effective_user_agent(
        configured_user_agent,
        original_user_agent,
    ))?;
    let mut builder = client
        .post(official_endpoint)
        .bearer_auth(&auth.access_token)
        .header("ChatGPT-Account-Id", &auth.account_id)
        .header("originator", "codex_cli_rs")
        .header("version", &client_version)
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    builder = session_headers.apply(builder, None);
    if is_stream {
        builder = builder
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header(reqwest::header::CACHE_CONTROL, "no-cache");
    }
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "protocol_proxy.official_request",
        json!({
            "route": "official_chatgpt",
            "endpoint": official_endpoint,
            "stream": is_stream,
            "candidateCount": 1,
            "willFailover": false
        }),
    );
    let mut upstream = send_upstream_request_for_responses(builder.json(&request_json), is_stream)
        .await
        .context("官方 ChatGPT 请求失败；已禁止回退到第三方供应商")?;
    let status_code = upstream.status().as_u16();
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mut prefetched_chunk = Vec::new();
    if settings.codex_app_capacity_retry && (200..300).contains(&status_code) && is_stream {
        match probe_stream_start_for_capacity_error(&mut upstream).await {
            Ok(StreamCapacityProbe::CapacityError(capacity_error_chunk)) => {
                let capacity_retry_attempt = capacity_retry_key.and_then(|key| {
                    next_capacity_retry_attempt(key, settings.codex_app_capacity_retry_max_attempts)
                });
                if capacity_retry_attempt.is_none() {
                    let _ = crate::diagnostic_log::append_diagnostic_log(
                        "protocol_proxy.official_stream_capacity_passthrough",
                        json!({
                            "route": "official_chatgpt",
                            "endpoint": official_endpoint,
                            "stream": true,
                            "statusCode": status_code,
                            "candidateCount": 1,
                            "maxAttempts": settings.codex_app_capacity_retry_max_attempts,
                            "reason": "selected_model_at_capacity"
                        }),
                    );
                    return Ok(UpstreamProxyResponse {
                        status_code,
                        content_type,
                        is_stream: true,
                        wire_api: UpstreamWireApi::Responses,
                        capacity_retryable: false,
                        capacity_retry_enabled: true,
                        capacity_retry_key,
                        capacity_retry_max_attempts: settings.codex_app_capacity_retry_max_attempts,
                        prefetched_chunk: capacity_error_chunk,
                        compaction: false,
                        response: upstream,
                        scheduled_account_id: None,
                        native_compaction_passthrough: false,
                        request_trace: None,
                        exec_adapted: false,
                        exec_outcome: None,
                        _channel_permit: None,
                    });
                }
                let capacity_retry_attempt = capacity_retry_attempt.unwrap_or_default();
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "protocol_proxy.official_stream_capacity_rewritten",
                    json!({
                        "route": "official_chatgpt",
                        "endpoint": official_endpoint,
                        "stream": true,
                        "statusCode": status_code,
                        "candidateCount": 1,
                        "capacityRetryAttempt": capacity_retry_attempt,
                        "capacityRetryMaxAttempts": settings.codex_app_capacity_retry_max_attempts,
                        "willFailover": false,
                        "reason": "selected_model_at_capacity"
                    }),
                );
                return Ok(UpstreamProxyResponse {
                    status_code: 503,
                    content_type: "application/json; charset=utf-8".to_string(),
                    is_stream: false,
                    wire_api: UpstreamWireApi::Responses,
                    capacity_retryable: true,
                    capacity_retry_enabled: true,
                    capacity_retry_key,
                    capacity_retry_max_attempts: settings.codex_app_capacity_retry_max_attempts,
                    prefetched_chunk: Vec::new(),
                    compaction: false,
                    response: upstream,
                    scheduled_account_id: None,
                    native_compaction_passthrough: false,
                    request_trace: None,
                    exec_adapted: false,
                    exec_outcome: None,
                    _channel_permit: None,
                });
            }
            Ok(StreamCapacityProbe::Prefetched(chunk)) => prefetched_chunk = chunk,
            Err(error) => {
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "protocol_proxy.official_stream_probe_failed",
                    json!({
                        "route": "official_chatgpt",
                        "stream": true,
                        "error": error.to_string()
                    }),
                );
                return Err(error).context("读取官方 ChatGPT 流首段失败");
            }
        }
    }
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "protocol_proxy.official_response",
        json!({
            "route": "official_chatgpt",
            "endpoint": official_endpoint,
            "stream": is_stream,
            "statusCode": status_code,
            "candidateCount": 1,
            "willFailover": false
        }),
    );
    if (200..300).contains(&status_code)
        && let Some(key) = capacity_retry_key
    {
        reset_capacity_retry_attempts(key);
    }
    Ok(UpstreamProxyResponse {
        status_code,
        is_stream: is_stream || content_type.contains("text/event-stream"),
        content_type,
        wire_api: UpstreamWireApi::Responses,
        capacity_retryable: false,
        capacity_retry_enabled: settings.codex_app_capacity_retry,
        capacity_retry_key,
        capacity_retry_max_attempts: settings.codex_app_capacity_retry_max_attempts,
        prefetched_chunk,
        compaction: false,
        response: upstream,
        scheduled_account_id: None,
        native_compaction_passthrough: false,
        request_trace: None,
        exec_adapted: false,
        exec_outcome: None,
        _channel_permit: None,
    })
}

pub(super) fn normalize_responses_input_items(request: &mut Value) {
    let Some(items) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    items.retain(|item| {
        item.get("type").and_then(Value::as_str) != Some("reasoning")
            || item
                .get("encrypted_content")
                .and_then(Value::as_str)
                .is_some_and(|content| !content.trim().is_empty())
    });
    for item in items {
        if item.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !id.starts_with("msg_") {
            item["id"] = json!(format!("msg_{id}"));
        }
    }
}

fn aggregate_upstream_model_override(
    settings: &crate::settings::BackendSettings,
    relay: &crate::settings::RelayProfile,
    requested_model: Option<&str>,
) -> Option<String> {
    let aggregate = settings.active_aggregate_relay_profile()?;
    let requested_model = requested_model.map(str::trim).unwrap_or("");
    let normalized_model =
        crate::aggregate_model_alias::normalize_requested_model_name(requested_model);
    if !requested_model.is_empty()
        && (relay.model_mappings.contains_key(requested_model)
            || relay.model_mappings.contains_key(&normalized_model))
    {
        return None;
    }
    // 聚合别名必须按实际选中的成员解析。不能用该成员的默认 model 覆盖
    // `deepseek:deepseek-v4-pro` 这类显式选择，否则多个模型会误发到同一个 API 模型。
    let members = aggregate
        .members
        .iter()
        .filter_map(|member| {
            settings
                .relay_profiles
                .iter()
                .find(|profile| profile.id == member.relay_id)
                .cloned()
        })
        .collect::<Vec<_>>();
    if let Some(alias) =
        crate::aggregate_model_alias::aggregate_catalog_aliases(&aggregate, &members)
            .into_iter()
            .find(|alias| alias.alias == requested_model && alias.provider_id == relay.id)
    {
        return Some(alias.target_model);
    }
    let model = crate::relay_config::relay_profile_model(relay);
    let model = model.trim();
    (!model.is_empty()).then(|| model.to_string())
}

pub async fn open_models_proxy_request(
    original_user_agent: Option<&str>,
) -> anyhow::Result<UpstreamProxyResponse> {
    let settings = SettingsStore::default().load().unwrap_or_default();
    let relay = crate::relay_rotation::select_relay_for_probe(&settings, None)?;
    validate_upstream(&relay)?;

    let endpoint = models_url(&relay.base_url);
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "protocol_proxy.models_request",
        json!({
            "relayId": relay.id,
            "relayName": relay.name,
            "endpoint": endpoint,
            "wireApi": UpstreamWireApi::Responses
        }),
    );
    let request = crate::http_client::proxied_client(&effective_user_agent(
        &relay.user_agent,
        original_user_agent,
    ))?
    .get(endpoint);
    let upstream = send_upstream_request(with_relay_auth(request, &relay)).await?;
    let status_code = upstream.status().as_u16();
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json; charset=utf-8")
        .to_string();

    Ok(UpstreamProxyResponse {
        status_code,
        is_stream: false,
        content_type,
        wire_api: UpstreamWireApi::Responses,
        capacity_retryable: false,
        capacity_retry_enabled: false,
        capacity_retry_key: None,
        capacity_retry_max_attempts: 0,
        prefetched_chunk: Vec::new(),
        compaction: false,

        response: upstream,
        scheduled_account_id: None,
        native_compaction_passthrough: false,
        request_trace: None,
        exec_adapted: false,
        exec_outcome: None,
        _channel_permit: None,
    })
}

pub async fn open_audio_transcriptions_proxy_request(
    body: &[u8],
    content_type: &str,
    original_user_agent: Option<&str>,
) -> anyhow::Result<UpstreamProxyResponse> {
    let settings = SettingsStore::default().load().unwrap_or_default();
    let relay = crate::relay_rotation::select_relay_for_probe(&settings, None)?;
    validate_upstream(&relay)?;
    let content_type = content_type.trim();
    if content_type.is_empty() {
        anyhow::bail!("Audio transcriptions 请求缺少 Content-Type");
    }

    let endpoint = audio_transcriptions_url(&relay.base_url);
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "protocol_proxy.audio_transcriptions_request",
        json!({
            "relayId": relay.id,
            "relayName": relay.name,
            "endpoint": endpoint,
            "wireApi": UpstreamWireApi::AudioTranscriptions,
            "bodyBytes": body.len()
        }),
    );
    let request = crate::http_client::proxied_client(&effective_user_agent(
        &relay.user_agent,
        original_user_agent,
    ))?
    .post(endpoint)
    .header(reqwest::header::CONTENT_TYPE, content_type)
    .body(body.to_vec());
    let upstream = send_upstream_request(with_relay_auth(request, &relay)).await?;
    let status_code = upstream.status().as_u16();
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json; charset=utf-8")
        .to_string();

    Ok(UpstreamProxyResponse {
        status_code,
        is_stream: false,
        content_type,
        wire_api: UpstreamWireApi::AudioTranscriptions,
        capacity_retryable: false,
        capacity_retry_enabled: false,
        capacity_retry_key: None,
        capacity_retry_max_attempts: 0,
        prefetched_chunk: Vec::new(),
        compaction: false,

        response: upstream,
        scheduled_account_id: None,
        native_compaction_passthrough: false,
        request_trace: None,
        exec_adapted: false,
        exec_outcome: None,
        _channel_permit: None,
    })
}

pub async fn open_image_generations_proxy_request(
    body: &[u8],
    original_user_agent: Option<&str>,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_image_proxy_request(
        body,
        "application/json",
        original_user_agent,
        ImageProxyEndpoint::Generations,
    )
    .await
}

pub async fn open_image_edits_proxy_request(
    body: &[u8],
    content_type: &str,
    original_user_agent: Option<&str>,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_image_proxy_request(
        body,
        content_type,
        original_user_agent,
        ImageProxyEndpoint::Edits,
    )
    .await
}

#[derive(Debug, Clone, Copy)]
enum ImageProxyEndpoint {
    Generations,
    Edits,
}

impl ImageProxyEndpoint {
    fn url(self, base_url: &str) -> String {
        match self {
            Self::Generations => image_generations_url(base_url),
            Self::Edits => image_edits_url(base_url),
        }
    }

    fn wire_api(self) -> UpstreamWireApi {
        match self {
            Self::Generations => UpstreamWireApi::ImageGenerations,
            Self::Edits => UpstreamWireApi::ImageEdits,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Generations => "image_generations",
            Self::Edits => "image_edits",
        }
    }
}

async fn open_image_proxy_request(
    body: &[u8],
    content_type: &str,
    original_user_agent: Option<&str>,
    endpoint_kind: ImageProxyEndpoint,
) -> anyhow::Result<UpstreamProxyResponse> {
    let settings = SettingsStore::default().load().unwrap_or_default();
    let relay = crate::relay_rotation::select_relay_for_probe(&settings, None)?;
    let base_url = if relay.upstream_base_url.trim().is_empty() {
        crate::relay_config::relay_profile_base_url(&relay)
    } else {
        relay.upstream_base_url.trim().to_string()
    };
    if is_local_protocol_proxy_base_url(&base_url) {
        anyhow::bail!("图片上游 Base URL 不能指向本地协议代理");
    }
    if base_url.trim().is_empty() {
        anyhow::bail!("图片上游 Base URL 不能为空");
    }
    if relay.api_key.trim().is_empty() && !relay.uses_no_auth() {
        anyhow::bail!("图片上游 Key 不能为空");
    }
    let content_type = content_type.trim();
    let content_type = if content_type.is_empty() {
        match endpoint_kind {
            ImageProxyEndpoint::Generations => "application/json",
            ImageProxyEndpoint::Edits => {
                anyhow::bail!("图片 edits 请求缺少 Content-Type");
            }
        }
    } else {
        content_type
    };
    let endpoint = endpoint_kind.url(&base_url);
    let wire_api = endpoint_kind.wire_api();
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "protocol_proxy.image_request",
        json!({
            "relayId": relay.id,
            "relayName": relay.name,
            "endpoint": endpoint,
            "wireApi": wire_api,
            "bodyBytes": body.len(),
            "endpointKind": endpoint_kind.name()
        }),
    );
    let request = crate::http_client::proxied_client(&effective_user_agent(
        &relay.user_agent,
        original_user_agent,
    ))?
    .post(endpoint)
    .header(reqwest::header::CONTENT_TYPE, content_type)
    .body(body.to_vec());
    let upstream = send_upstream_request_with_header_timeout(
        with_relay_auth(request, &relay),
        UPSTREAM_IMAGE_HEADER_TIMEOUT,
    )
    .await?;
    let status_code = upstream.status().as_u16();
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json; charset=utf-8")
        .to_string();

    Ok(UpstreamProxyResponse {
        status_code,
        is_stream: false,
        content_type,
        wire_api,
        capacity_retryable: false,
        capacity_retry_enabled: false,
        capacity_retry_key: None,
        capacity_retry_max_attempts: 0,
        prefetched_chunk: Vec::new(),
        compaction: false,

        response: upstream,
        scheduled_account_id: None,
        native_compaction_passthrough: false,
        request_trace: None,
        exec_adapted: false,
        exec_outcome: None,
        _channel_permit: None,
    })
}

fn response_header_timeout(is_stream: bool) -> Duration {
    if is_stream {
        UPSTREAM_STREAM_HEADER_TIMEOUT
    } else {
        UPSTREAM_HEADER_TIMEOUT
    }
}

enum StreamCapacityProbe {
    CapacityError(Vec<u8>),
    Prefetched(Vec<u8>),
}

async fn probe_stream_start_for_capacity_error(
    response: &mut reqwest::Response,
) -> anyhow::Result<StreamCapacityProbe> {
    let mut prefetched = Vec::new();

    while prefetched.len() < STREAM_CAPACITY_PROBE_LIMIT {
        let chunk = tokio::time::timeout(upstream_stream_idle_timeout(), response.chunk())
            .await
            .with_context(|| {
                format!(
                    "上游流连续 {} 秒没有返回数据",
                    upstream_stream_idle_timeout().as_secs()
                )
            })?
            .context("读取上游流失败")?;
        let Some(chunk) = chunk else {
            break;
        };
        prefetched.extend_from_slice(&chunk);
        if is_selected_model_capacity_error(&prefetched) {
            return Ok(StreamCapacityProbe::CapacityError(prefetched));
        }
        if stream_prefix_contains_normal_progress(&prefetched)
            || stream_prefix_contains_complete_json(&prefetched)
        {
            break;
        }
    }

    if prefetched.len() >= STREAM_CAPACITY_PROBE_LIMIT {
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "protocol_proxy.stream_capacity_probe_limit_reached",
            json!({
                "prefetchedBytes": prefetched.len(),
                "limitBytes": STREAM_CAPACITY_PROBE_LIMIT,
            }),
        );
    }

    Ok(StreamCapacityProbe::Prefetched(prefetched))
}

pub fn is_selected_model_capacity_error(payload: &[u8]) -> bool {
    let text = String::from_utf8_lossy(payload);
    if let Ok(value) = serde_json::from_str::<Value>(text.trim())
        && json_value_is_capacity_error(&value)
    {
        return true;
    }

    let mut buffer = text.replace("\r\n", "\n");
    let mut saw_sse_block = false;
    while let Some(block) = take_sse_block(&mut buffer) {
        saw_sse_block = true;
        if sse_block_is_capacity_error(&block) {
            return true;
        }
    }

    let lower = text.to_ascii_lowercase();
    let has_marker = text_contains_capacity_marker(&lower);
    if !saw_sse_block {
        return has_marker;
    }

    has_marker
        && (lower.contains("event: error")
            || lower.contains("response.failed")
            || lower.contains("\"error\"")
            || lower.contains("\"status\":\"failed\""))
}

fn sse_block_is_capacity_error(block: &str) -> bool {
    let mut event_name = None;
    let mut data_parts = Vec::new();
    for line in block.lines() {
        if let Some(event) = strip_sse_field(line, "event") {
            event_name = Some(event.trim());
        }
        if let Some(data) = strip_sse_field(line, "data") {
            data_parts.push(data);
        }
    }
    let data = data_parts.join("\n");
    let event_is_error = event_name.is_some_and(is_responses_error_event);
    if let Ok(value) = serde_json::from_str::<Value>(&data) {
        return (event_is_error || json_value_is_error_envelope(&value))
            && value_contains_capacity_marker(&value);
    }
    event_is_error && text_contains_capacity_marker(&data.to_ascii_lowercase())
}

fn json_value_is_capacity_error(value: &Value) -> bool {
    json_value_is_error_envelope(value) && value_contains_capacity_marker(value)
}

fn json_value_is_error_envelope(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.get("error").is_some() {
        return true;
    }
    if object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(is_responses_error_event)
        || object
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| status.eq_ignore_ascii_case("failed"))
    {
        return true;
    }
    object
        .get("response")
        .is_some_and(json_value_is_error_envelope)
}

fn is_responses_error_event(event: &str) -> bool {
    event.eq_ignore_ascii_case("error") || event.eq_ignore_ascii_case("response.failed")
}

fn value_contains_capacity_marker(value: &Value) -> bool {
    match value {
        Value::String(value) => text_contains_capacity_marker(&value.to_ascii_lowercase()),
        Value::Array(values) => values.iter().any(value_contains_capacity_marker),
        Value::Object(values) => values.values().any(value_contains_capacity_marker),
        _ => false,
    }
}

fn text_contains_capacity_marker(lower: &str) -> bool {
    lower.contains("selected model is at capacity")
        || lower.contains("model is at capacity")
        || lower.contains("model_at_capacity")
        || lower.contains("model-at-capacity")
        || lower.contains("model_capacity")
        || lower.contains("model-capacity")
        || (lower.contains("capacity") && lower.contains("different model"))
}

fn stream_prefix_contains_complete_event(payload: &[u8]) -> bool {
    payload.windows(2).any(|window| window == b"\n\n")
        || payload.windows(4).any(|window| window == b"\r\n\r\n")
}

fn stream_prefix_contains_normal_progress(payload: &[u8]) -> bool {
    if !stream_prefix_contains_complete_event(payload) {
        return false;
    }
    let mut buffer = String::from_utf8_lossy(payload).into_owned();
    while let Some(block) = take_sse_block(&mut buffer) {
        let mut event_name = None;
        let mut data_parts = Vec::new();
        for line in block.lines() {
            if let Some(event) = strip_sse_field(line, "event") {
                event_name = Some(event.trim());
            }
            if let Some(data) = strip_sse_field(line, "data") {
                data_parts.push(data);
            }
        }
        let data = data_parts.join("\n");
        let parsed_data = serde_json::from_str::<Value>(&data).ok();
        let data_type = parsed_data.as_ref().and_then(|value| {
            value
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        let event_type = event_name.or(data_type.as_deref());
        if data.trim() == "[DONE]" {
            return true;
        }
        if event_type.is_some_and(is_safe_stream_progress_event) {
            return true;
        }
        // Chat Completions streams do not carry a Responses event type, but
        // a complete choices object means actual output has started.
        if parsed_data
            .as_ref()
            .is_some_and(|value| value.get("choices").is_some())
        {
            return true;
        }
        if event_type.is_none()
            && block.lines().any(|line| {
                let line = line.trim();
                !line.is_empty() && !line.starts_with(':')
            })
        {
            return true;
        }
    }
    false
}

fn is_safe_stream_progress_event(event: &str) -> bool {
    matches!(
        event,
        "response.completed"
            | "response.failed"
            | "response.incomplete"
            | "response.output_text.delta"
            | "response.output_text.done"
            | "response.refusal.delta"
            | "response.refusal.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.function_call_arguments.delta"
            | "response.function_call_arguments.done"
            | "response.custom_tool_call_input.delta"
            | "response.custom_tool_call_input.done"
            | "response.audio.delta"
            | "response.audio.done"
            | "response.audio_transcript.delta"
            | "response.audio_transcript.done"
            | "response.code_interpreter_call_code.delta"
            | "response.code_interpreter_call_code.done"
            | "error"
            | "message"
    )
}

fn stream_prefix_contains_complete_json(payload: &[u8]) -> bool {
    let trimmed = payload
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .collect::<Vec<_>>();
    matches!(trimmed.first(), Some(b'{') | Some(b'['))
        && serde_json::from_slice::<Value>(&trimmed).is_ok()
}

pub async fn open_chat_completions_proxy_request(
    body: &str,
    original_user_agent: Option<&str>,
) -> anyhow::Result<UpstreamProxyResponse> {
    let settings = SettingsStore::default().load().unwrap_or_default();
    let relay = settings.active_relay_profile();
    if relay.protocol != RelayProtocol::ChatCompletions {
        anyhow::bail!("当前中转未启用 Chat Completions 协议代理");
    }
    if relay.base_url.trim().is_empty() {
        anyhow::bail!("Chat Completions 上游 Base URL 不能为空");
    }
    if relay.api_key.trim().is_empty() && !relay.uses_no_auth() {
        anyhow::bail!("Chat Completions 上游 Key 不能为空");
    }

    let request_json: Value = serde_json::from_str(body)?;
    let is_stream = request_json
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let channel_key = crate::channel_protection::key_for_relay(&relay);
    let mut cooldown_retries = 0_usize;
    loop {
        let channel_permit = crate::channel_protection::acquire(&channel_key, &relay).await;
        let mut request = crate::http_client::proxied_client(&effective_user_agent(
            &relay.user_agent,
            original_user_agent,
        ))?
        .post(chat_completions_url(&relay.base_url))
        .header(reqwest::header::CONTENT_TYPE, "application/json");
        if is_stream {
            request = request
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .header(reqwest::header::CACHE_CONTROL, "no-cache");
        }
        let upstream = match with_relay_auth(request.json(&request_json), &relay)
            .send()
            .await
        {
            Ok(upstream) => upstream,
            Err(error) => {
                drop(channel_permit);
                return Err(error.into());
            }
        };
        let status_code = upstream.status().as_u16();
        let retry_after = crate::channel_protection::retry_after_duration(upstream.headers());
        if !(200..300).contains(&status_code) {
            let cooldown_started = crate::channel_protection::mark_failure(
                &channel_key,
                &relay,
                status_code,
                retry_after,
            )
            .await;
            if cooldown_started && should_retry_after_cooldown(cooldown_retries) {
                cooldown_retries = cooldown_retries.saturating_add(1);
                drop(channel_permit);
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "protocol_proxy.channel_cooldown_retry",
                    json!({
                        "relayId": relay.id,
                        "relayName": relay.name,
                        "statusCode": status_code,
                        "retryAfterSeconds": retry_after.map(|value| value.as_secs()),
                        "retry": cooldown_retries
                    }),
                );
                continue;
            }
        }
        let content_type = upstream
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();

        return Ok(UpstreamProxyResponse {
            status_code,
            is_stream: is_stream || content_type.contains("text/event-stream"),
            content_type,
            wire_api: UpstreamWireApi::ChatCompletions,
            capacity_retryable: false,
            capacity_retry_enabled: settings.codex_app_capacity_retry,
            capacity_retry_key: settings
                .codex_app_capacity_retry
                .then(|| capacity_retry_request_key(&request_json)),
            capacity_retry_max_attempts: settings.codex_app_capacity_retry_max_attempts,
            prefetched_chunk: Vec::new(),
            compaction: false,
            response: upstream,
            scheduled_account_id: None,
            native_compaction_passthrough: false,
            request_trace: None,
            exec_adapted: false,
            exec_outcome: None,
            _channel_permit: Some(channel_permit),
        });
    }
}

async fn upstream_request_parts(
    relay: &crate::settings::RelayProfile,
    mut request_json: Value,
    request_path: &str,
    model_override: Option<&str>,
) -> anyhow::Result<(String, Value, UpstreamWireApi, bool)> {
    let compact = is_responses_compact_proxy_path(request_path)
        || request_has_compaction_trigger(&request_json);
    let is_v2_compaction = compact && request_has_compaction_trigger(&request_json);
    // 原生 Responses 状态必须来自上游，不能以普通摘要冒充加密状态。
    let synthetic_compaction = compact && relay.protocol == RelayProtocol::ChatCompletions;
    if synthetic_compaction {
        request_json = rewrite_request_for_compaction(strip_compaction_trigger(request_json));
    }
    if let Some(model) = model_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request_json["model"] = json!(model);
    }
    if relay.protocol == RelayProtocol::AnthropicMessages {
        let (endpoint, body, wire, compact) = super::upstream_request_parts(
            relay,
            request_json,
            request_path,
            model_override,
            false,
            None,
            super::ProxySessionHeaders::default(),
        )
        .await?;
        debug_assert_eq!(wire, super::UpstreamWireApi::AnthropicMessages);
        return Ok((endpoint, body, UpstreamWireApi::AnthropicMessages, compact));
    }
    let mut body = match relay.protocol {
        RelayProtocol::AnthropicMessages => unreachable!("handled above"),
        RelayProtocol::Responses => request_json,
        RelayProtocol::ChatCompletions => responses_to_chat_completions_with_options(
            request_json,
            relay.standard_openai_protocol,
        )?,
    };
    rewrite_model_for_relay(&mut body, relay);
    if relay.protocol == RelayProtocol::Responses {
        normalize_responses_input_items(&mut body);
        normalize_responses_item_ids(&mut body);
        if relay.web_search_history_compat_enabled()
            && !is_responses_compact_proxy_path(request_path)
        {
            super::ensure_web_search_tool_for_history(&mut body);
        }
    }

    // Image handling (per-model): send-as-is / strip / VLM analysis
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !model.is_empty() {
        use crate::vision::ImageHandling;
        match crate::vision::image_handling_mode(&model, &relay.model_vlm) {
            ImageHandling::SendAsIs => { /* 不做任何处理 */ }
            ImageHandling::Strip => {
                for key in &["messages", "input"] {
                    if let Some(arr) = body.get_mut(key).and_then(Value::as_array_mut) {
                        crate::vision::strip_images_only(arr);
                    }
                }
            }
            ImageHandling::Vlm => {
                if !relay.vlm_api_key.is_empty()
                    && !relay.vlm_model.is_empty()
                    && !relay.vlm_base_url.is_empty()
                {
                    let vlm_config = crate::vision::VlmConfig {
                        api_key: relay.vlm_api_key.clone(),
                        model: relay.vlm_model.clone(),
                        base_url: relay.vlm_base_url.clone(),
                    };

                    for key in &["messages", "input"] {
                        if let Some(arr) = body.get_mut(key).and_then(Value::as_array_mut) {
                            crate::vision::strip_image_blocks(
                                arr,
                                &vlm_config,
                                &relay.model_windows,
                                &relay.context_window,
                                &model,
                                relay.protocol == crate::settings::RelayProtocol::Responses,
                            )
                            .await?;
                        }
                    }
                }
            }
        }
    }

    if guard_inline_image_data_urls(&mut body) {
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "inline_image_data_url_leak",
            json!({ "model": model, "protocol": format!("{:?}", relay.protocol) }),
        );
        debug_assert!(false, "base64 图片泄漏进文本字段，检查协议转换路径");
    }

    let wire_api = match relay.protocol {
        RelayProtocol::AnthropicMessages => UpstreamWireApi::AnthropicMessages,
        RelayProtocol::Responses => UpstreamWireApi::Responses,
        RelayProtocol::ChatCompletions => UpstreamWireApi::ChatCompletions,
    };
    Ok((
        match relay.protocol {
            RelayProtocol::AnthropicMessages => {
                unreachable!("handled by upstream Anthropic adapter")
            }
            // v2 压缩请求走普通 /responses 端点（compaction_trigger 在 input 里）；
            // 显式 legacy compact 保留原端点。
            RelayProtocol::Responses if compact && !is_v2_compaction => {
                responses_compact_url(&relay.base_url)
            }
            RelayProtocol::Responses => responses_url(&relay.base_url),
            RelayProtocol::ChatCompletions => chat_completions_url(&relay.base_url),
        },
        body,
        wire_api,
        synthetic_compaction,
    ))
}

fn rewrite_model_for_relay(body: &mut Value, relay: &crate::settings::RelayProfile) {
    let Some(requested_model) = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return;
    };
    let normalized_model =
        crate::aggregate_model_alias::normalize_requested_model_name(requested_model);
    let target_model = relay
        .model_mappings
        .get(requested_model)
        .or_else(|| relay.model_mappings.get(&normalized_model))
        .map(String::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty());
    if let Some(target_model) = target_model.filter(|model| *model != requested_model) {
        body["model"] = Value::String(target_model.to_string());
    }
}

fn upstream_request_builder(
    client: reqwest::Client,
    endpoint: &str,
    relay: &crate::settings::RelayProfile,
    is_stream: bool,
    upstream_body: &Value,
) -> reqwest::RequestBuilder {
    let mut builder = client
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    builder = with_relay_auth(builder, relay);
    if is_stream {
        builder = builder
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header(reqwest::header::CACHE_CONTROL, "no-cache");
    }
    builder.json(upstream_body)
}

fn validate_upstream(relay: &crate::settings::RelayProfile) -> anyhow::Result<()> {
    if relay.base_url.trim().is_empty() {
        anyhow::bail!("上游 Base URL 不能为空");
    }
    if relay.api_key.trim().is_empty() && !relay.uses_no_auth() {
        anyhow::bail!("上游 Key 不能为空");
    }
    Ok(())
}

fn with_relay_auth(
    request: reqwest::RequestBuilder,
    relay: &crate::settings::RelayProfile,
) -> reqwest::RequestBuilder {
    // 认证（API Key / 无认证）+ 供应商自定义请求头，统一在 relay_headers 里决定优先级。
    crate::relay_headers::apply_for_protocol(request, relay)
}

fn conversation_id_from_responses_request(body: &Value) -> Option<String> {
    for key in ["conversation", "conversation_id", "previous_response_id"] {
        if let Some(value) = body.get(key).and_then(Value::as_str) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn effective_user_agent(configured_user_agent: &str, original_user_agent: Option<&str>) -> String {
    let configured_user_agent = configured_user_agent.trim();
    if !configured_user_agent.is_empty() {
        return configured_user_agent.to_string();
    }
    original_user_agent
        .map(str::trim)
        .filter(|user_agent| !user_agent.is_empty())
        .unwrap_or("")
        .to_string()
}

pub async fn handle_responses_proxy_request(body: &str) -> anyhow::Result<ProxyHttpResponse> {
    let request_json: Value = serde_json::from_str(body)?;
    let mut retry_round = 0u8;
    let upstream = loop {
        let upstream = open_responses_proxy_request_with_capacity_retries(body, None).await?;
        if upstream.capacity_retryable {
            retry_round = retry_round.saturating_add(1);
            tokio::time::sleep(capacity_retry_backoff(retry_round)).await;
            continue;
        }
        if upstream.is_success() {
            break upstream;
        }

        let status_code = upstream.status_code;
        let capacity_retry_enabled = upstream.capacity_retry_enabled;
        let capacity_retry_key = upstream.capacity_retry_key;
        let capacity_retry_max_attempts = upstream.capacity_retry_max_attempts;
        let upstream_content_type = upstream.content_type.clone();
        let mut upstream_body = upstream.prefetched_chunk;
        upstream_body.extend_from_slice(
            &read_upstream_body_with_timeout(upstream.response, upstream_body_timeout()).await?,
        );
        let is_capacity =
            capacity_retry_enabled && is_selected_model_capacity_error(&upstream_body);
        if is_capacity
            && capacity_retry_key.is_some_and(|key| {
                next_capacity_retry_attempt(key, capacity_retry_max_attempts).is_some()
            })
        {
            retry_round = retry_round.saturating_add(1);
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "protocol_proxy.capacity_retry_loop",
                json!({
                    "source": "handle_responses_proxy_request",
                    "attempt": retry_round,
                    "maxAttempts": capacity_retry_max_attempts,
                    "willRetry": true,
                    "reason": "selected_model_at_capacity"
                }),
            );
            tokio::time::sleep(capacity_retry_backoff(retry_round)).await;
            continue;
        }
        if !is_capacity {
            if let Some(key) = capacity_retry_key {
                reset_capacity_retry_attempts(key);
            }
        }
        let error =
            responses_error_from_upstream(status_code, &upstream_content_type, &upstream_body);
        return Ok(ProxyHttpResponse {
            status: http_status_line(status_code),
            content_type: "application/json; charset=utf-8".to_string(),
            body: serde_json::to_vec(&error)?,
        });
    };
    let is_compaction = upstream.compaction;
    let upstream_content_type = upstream.content_type.clone();
    let is_stream = upstream.is_stream;
    let wire_api = upstream.wire_api;
    let mut upstream_body = upstream.prefetched_chunk;
    upstream_body.extend_from_slice(
        &read_upstream_body_with_timeout(upstream.response, upstream_body_timeout()).await?,
    );

    if is_compaction {
        // v2 压缩：无论上游协议/是否流式，都重组为单个 compaction 输出项。
        let model = request_json
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !is_stream {
            return Ok(ProxyHttpResponse {
                status: "200 OK".to_string(),
                content_type: "text/event-stream; charset=utf-8".to_string(),
                body: wrap_non_stream_response_as_compaction(&upstream_body, model)?,
            });
        }
        let mut converter = CompactionSseConverter::new(model);
        if wire_api != UpstreamWireApi::Responses {
            converter = converter.with_chat_upstream();
        }
        // 整包已收齐，直接喂给有状态 SSE 解析器（与 launcher 逐 chunk 路径同逻辑）。
        converter.push_upstream_bytes(&upstream_body);
        return Ok(ProxyHttpResponse {
            status: "200 OK".to_string(),
            content_type: "text/event-stream; charset=utf-8".to_string(),
            body: converter.finish(),
        });
    }

    if wire_api == UpstreamWireApi::Responses {
        return Ok(ProxyHttpResponse {
            status: "200 OK".to_string(),
            content_type: if upstream_content_type.is_empty() {
                "application/json; charset=utf-8".to_string()
            } else {
                upstream_content_type
            },
            body: upstream_body.to_vec(),
        });
    }

    if is_stream {
        let text = String::from_utf8_lossy(&upstream_body);
        return Ok(ProxyHttpResponse {
            status: "200 OK".to_string(),
            content_type: "text/event-stream; charset=utf-8".to_string(),
            body: chat_sse_to_responses_sse_with_request(&text, &request_json).into_bytes(),
        });
    }

    let chat_json: Value = serde_json::from_slice(&upstream_body)?;
    let response_json = chat_completion_to_response_with_request(chat_json, &request_json)?;
    Ok(ProxyHttpResponse {
        status: "200 OK".to_string(),
        content_type: "application/json; charset=utf-8".to_string(),
        body: serde_json::to_vec(&response_json)?,
    })
}

pub fn chat_completions_url(base_url: &str) -> String {
    let skip_version_prefix = base_url.trim().ends_with('#');
    let base = base_url.trim().trim_end_matches('#').trim_end_matches('/');
    if base.to_ascii_lowercase().ends_with("/chat/completions") {
        return base.to_string();
    }
    let origin_only = base
        .split_once("://")
        .map_or(!base.contains('/'), |(_, rest)| !rest.contains('/'));
    let mut url = if skip_version_prefix || has_version_suffix(base) || !origin_only {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    };
    while url.contains("/v1/v1") {
        url = url.replace("/v1/v1", "/v1");
    }
    url
}

pub fn responses_url(base_url: &str) -> String {
    let skip_version_prefix = base_url.trim().ends_with('#');
    let base = base_url.trim().trim_end_matches('#').trim_end_matches('/');
    if base.to_ascii_lowercase().ends_with("/responses") {
        return base.to_string();
    }
    let origin_only = base
        .split_once("://")
        .map_or(!base.contains('/'), |(_, rest)| !rest.contains('/'));
    let mut url = if skip_version_prefix || has_version_suffix(base) || !origin_only {
        format!("{base}/responses")
    } else {
        format!("{base}/v1/responses")
    };
    while url.contains("/v1/v1") {
        url = url.replace("/v1/v1", "/v1");
    }
    url
}

pub fn responses_compact_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('#').trim_end_matches('/');
    if base.to_ascii_lowercase().ends_with("/responses/compact") {
        return base.to_string();
    }
    format!("{}/compact", responses_url(base_url).trim_end_matches('/'))
}

pub fn audio_transcriptions_url(base_url: &str) -> String {
    let skip_version_prefix = base_url.trim().ends_with('#');
    let base = base_url.trim().trim_end_matches('#').trim_end_matches('/');
    if base.to_ascii_lowercase().ends_with("/audio/transcriptions") {
        return base.to_string();
    }
    let origin_only = base
        .split_once("://")
        .map_or(!base.contains('/'), |(_, rest)| !rest.contains('/'));
    let mut url = if skip_version_prefix || has_version_suffix(base) || !origin_only {
        format!("{base}/audio/transcriptions")
    } else {
        format!("{base}/v1/audio/transcriptions")
    };
    while url.contains("/v1/v1") {
        url = url.replace("/v1/v1", "/v1");
    }
    url
}

pub fn image_generations_url(base_url: &str) -> String {
    image_endpoint_url(base_url, "generations")
}

pub fn image_edits_url(base_url: &str) -> String {
    image_endpoint_url(base_url, "edits")
}

fn image_endpoint_url(base_url: &str, endpoint: &str) -> String {
    let skip_version_prefix = base_url.trim().ends_with('#');
    let base = base_url.trim().trim_end_matches('#').trim_end_matches('/');
    if base
        .to_ascii_lowercase()
        .ends_with(&format!("/images/{endpoint}"))
    {
        return base.to_string();
    }
    let origin_only = base
        .split_once("://")
        .map_or(!base.contains('/'), |(_, rest)| !rest.contains('/'));
    let mut url = if skip_version_prefix || has_version_suffix(base) || !origin_only {
        format!("{base}/images/{endpoint}")
    } else {
        format!("{base}/v1/images/{endpoint}")
    };
    while url.contains("/v1/v1") {
        url = url.replace("/v1/v1", "/v1");
    }
    url
}

fn is_local_protocol_proxy_base_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else {
        return false;
    };
    if !url.scheme().eq_ignore_ascii_case("http") || url.port() != Some(protocol_proxy_port()) {
        return false;
    }
    matches!(
        url.host_str(),
        Some("127.0.0.1" | "localhost" | "::1" | "[::1]")
    )
}

#[cfg(test)]
mod image_proxy_tests {
    use super::UPSTREAM_IMAGE_HEADER_TIMEOUT;
    use super::is_local_protocol_proxy_base_url;
    use std::time::Duration;

    #[test]
    fn image_requests_allow_ten_minutes_for_response_headers() {
        assert_eq!(UPSTREAM_IMAGE_HEADER_TIMEOUT, Duration::from_secs(600));
    }

    #[test]
    fn local_protocol_proxy_detection_covers_common_loopback_forms() {
        for base_url in [
            "http://127.0.0.1:57321",
            "http://127.0.0.1:57321/",
            "http://127.0.0.1:57321/v1",
            "http://localhost:57321/v1/",
            "http://[::1]:57321/v1",
        ] {
            assert!(is_local_protocol_proxy_base_url(base_url), "{base_url}");
        }

        for base_url in [
            "https://127.0.0.1:57321/v1",
            "http://127.0.0.1:57322/v1",
            "http://api.example.test:57321/v1",
            "not-a-url",
        ] {
            assert!(!is_local_protocol_proxy_base_url(base_url), "{base_url}");
        }
    }
}

pub fn models_url(base_url: &str) -> String {
    let skip_version_prefix = base_url.trim().ends_with('#');
    let mut base = base_url
        .trim()
        .trim_end_matches('#')
        .trim_end_matches('/')
        .to_string();
    if base.to_ascii_lowercase().ends_with("/chat/completions") {
        base.truncate(base.len() - "/chat/completions".len());
    }
    if base.to_ascii_lowercase().ends_with("/models") {
        return base;
    }
    let origin_only = base
        .split_once("://")
        .map_or(!base.contains('/'), |(_, rest)| !rest.contains('/'));
    let mut url = if skip_version_prefix || has_version_suffix(&base) || !origin_only {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    };
    while url.contains("/v1/v1") {
        url = url.replace("/v1/v1", "/v1");
    }
    url
}

pub(crate) fn has_version_suffix(base_url: &str) -> bool {
    let segment = base_url.rsplit('/').next().unwrap_or(base_url);
    let Some(rest) = segment.strip_prefix('v') else {
        return false;
    };
    rest.chars().next().is_some_and(|ch| ch.is_ascii_digit())
}

pub fn chat_sse_to_responses_sse(input: &str) -> String {
    let mut converter = ChatSseToResponsesConverter::default();
    let mut output = converter.push_bytes(input.as_bytes());
    output.extend(converter.finish());
    String::from_utf8(output).unwrap_or_default()
}

pub fn chat_sse_to_responses_sse_with_request(input: &str, original_request: &Value) -> String {
    let mut converter = ChatSseToResponsesConverter::with_request(original_request);
    let mut output = converter.push_bytes(input.as_bytes());
    output.extend(converter.finish());
    String::from_utf8(output).unwrap_or_default()
}

pub fn response_id_from_chat_id(id: Option<&str>) -> String {
    let id = id.unwrap_or("compat");
    if id.starts_with("resp_") {
        id.to_string()
    } else {
        format!("resp_{id}")
    }
}

/// `resp_xxx` → `xxx`。message item 的 id 必须以 `msg_` 开头，
/// 直接拼在 `response_id` 后面会得到上游拒收的 `resp_xxx_msg`（#1431）。
fn response_id_body(response_id: &str) -> &str {
    response_id
        .strip_prefix("resp_")
        .filter(|value| !value.is_empty())
        .unwrap_or(response_id)
}

fn push_sse(output: &mut String, event: &str, data: Value) {
    output.push_str("event: ");
    output.push_str(event);
    output.push_str("\ndata: ");
    output.push_str(&serde_json::to_string(&data).unwrap_or_default());
    output.push_str("\n\n");
}

#[derive(Debug, Default)]
struct TextItemState {
    output_index: Option<u32>,
    item_id: String,
    text: String,
    added: bool,
    done: bool,
}

#[derive(Debug, Default)]
struct ReasoningItemState {
    output_index: Option<u32>,
    item_id: String,
    text: String,
    added: bool,
    done: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum InlineThinkMode {
    #[default]
    Detecting,
    Reasoning,
    Text,
}

#[derive(Debug, Default)]
struct InlineThinkState {
    mode: InlineThinkMode,
    buffer: String,
}

#[derive(Debug, Default)]
struct ToolCallState {
    output_index: Option<u32>,
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    added: bool,
    done: bool,
}

#[derive(Debug)]
struct ChatSseState {
    response_started: bool,
    completed: bool,
    response_id: String,
    model: String,
    created_at: u64,
    next_output_index: u32,
    text: TextItemState,
    reasoning: ReasoningItemState,
    inline_think: InlineThinkState,
    tools: BTreeMap<usize, ToolCallState>,
    output_items: Vec<(u32, Value)>,
    latest_usage: Option<Value>,
    finish_reason: Option<String>,
    tool_context: CodexToolContext,
    original_request: Option<Value>,
}

impl Default for ChatSseState {
    fn default() -> Self {
        Self {
            response_started: false,
            completed: false,
            response_id: "resp_compat".to_string(),
            model: String::new(),
            created_at: 0,
            next_output_index: 0,
            text: TextItemState::default(),
            reasoning: ReasoningItemState::default(),
            inline_think: InlineThinkState::default(),
            tools: BTreeMap::new(),
            output_items: Vec::new(),
            latest_usage: None,
            finish_reason: None,
            tool_context: CodexToolContext::default(),
            original_request: None,
        }
    }
}

impl ChatSseState {
    fn with_request(original_request: &Value) -> Self {
        let model = original_request
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut tool_context = build_codex_tool_context(original_request.get("tools"));
        for namespace_tool in &collect_tool_search_output_namespaces(original_request) {
            add_namespace_tools_to_context(&mut tool_context, namespace_tool);
        }
        Self {
            model,
            tool_context,

            original_request: Some(original_request.clone()),
            ..Self::default()
        }
    }

    fn handle_chat_chunk_into(&mut self, chunk: &Value, output: &mut String) {
        if let Some(id) = chunk.get("id").and_then(Value::as_str) {
            self.response_id = response_id_from_chat_id(Some(id));
        }
        if let Some(model) = chunk.get("model").and_then(Value::as_str) {
            if !model.is_empty() {
                self.model = model.to_string();
            }
        }
        if let Some(created) = chunk.get("created").and_then(Value::as_u64) {
            self.created_at = created;
        }
        self.ensure_response_started_into(output);

        if let Some(usage) = chunk.get("usage").filter(|value| !value.is_null()) {
            self.latest_usage = Some(chat_usage_to_responses_usage(Some(usage)));
        }

        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };

        if let Some(delta) = choice.get("delta") {
            if let Some(reasoning) = chat_delta_reasoning_text(delta) {
                self.push_reasoning_delta_into(&reasoning, output);
            }

            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                if !content.is_empty() {
                    self.push_content_delta_into(content, output);
                }
            }

            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                self.flush_inline_think_at_boundary_into(output);
                self.finalize_reasoning_into(output);
                for tool_call in tool_calls {
                    self.push_tool_call_delta_into(tool_call, output);
                }
            }
        }

        if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(finish_reason.to_string());
        }
    }

    fn push_content_delta_into(&mut self, delta: &str, output: &mut String) {
        match self.inline_think.mode {
            InlineThinkMode::Text => {
                self.finalize_reasoning_into(output);
                self.push_text_delta_into(delta, output);
            }
            InlineThinkMode::Detecting => {
                self.inline_think.buffer.push_str(delta);
                match leading_think_prefix_decision(&self.inline_think.buffer) {
                    ThinkPrefixDecision::NeedMore => {}
                    ThinkPrefixDecision::Reasoning => {
                        self.inline_think.mode = InlineThinkMode::Reasoning;
                        self.drain_complete_inline_think_into(output);
                    }
                    ThinkPrefixDecision::Text => {
                        self.inline_think.mode = InlineThinkMode::Text;
                        let text = std::mem::take(&mut self.inline_think.buffer);
                        self.finalize_reasoning_into(output);
                        self.push_text_delta_into(&text, output);
                    }
                }
            }
            InlineThinkMode::Reasoning => {
                self.inline_think.buffer.push_str(delta);
                self.drain_complete_inline_think_into(output);
            }
        }
    }

    fn drain_complete_inline_think_into(&mut self, output: &mut String) {
        let Some((reasoning, answer)) = split_leading_think_block(&self.inline_think.buffer) else {
            return;
        };
        self.inline_think.mode = InlineThinkMode::Text;
        self.inline_think.buffer.clear();
        if !reasoning.is_empty() {
            self.push_reasoning_delta_into(&reasoning, output);
            self.finalize_reasoning_into(output);
        }
        if !answer.is_empty() {
            self.push_text_delta_into(&answer, output);
        }
    }

    fn flush_inline_think_at_boundary_into(&mut self, output: &mut String) {
        match self.inline_think.mode {
            InlineThinkMode::Text => {}
            InlineThinkMode::Detecting => {
                self.inline_think.mode = InlineThinkMode::Text;
                let text = std::mem::take(&mut self.inline_think.buffer);
                if !text.is_empty() {
                    self.finalize_reasoning_into(output);
                    self.push_text_delta_into(&text, output);
                }
            }
            InlineThinkMode::Reasoning => {
                let buffered = std::mem::take(&mut self.inline_think.buffer);
                self.inline_think.mode = InlineThinkMode::Text;
                if let Some((reasoning, answer)) = split_leading_think_block(&buffered) {
                    if !reasoning.is_empty() {
                        self.push_reasoning_delta_into(&reasoning, output);
                        self.finalize_reasoning_into(output);
                    }
                    if !answer.is_empty() {
                        self.push_text_delta_into(&answer, output);
                    }
                    return;
                }
                let reasoning = strip_leading_think_open_tag(&buffered).unwrap_or(buffered);
                if !reasoning.is_empty() {
                    self.push_reasoning_delta_into(&reasoning, output);
                    self.finalize_reasoning_into(output);
                }
            }
        }
    }

    fn ensure_response_started_into(&mut self, output: &mut String) {
        if self.response_started {
            return;
        }
        self.response_started = true;
        push_sse(
            output,
            "response.created",
            json!({
                "type": "response.created",
                "response": self.base_response("in_progress", Vec::new())
            }),
        );
        push_sse(
            output,
            "response.in_progress",
            json!({
                "type": "response.in_progress",
                "response": self.base_response("in_progress", Vec::new())
            }),
        );
    }

    fn push_reasoning_delta_into(&mut self, delta: &str, output: &mut String) {
        if !self.reasoning.added {
            let output_index = self.next_output_index();
            let item_id = format!("rs_{}", self.response_id);
            self.reasoning.output_index = Some(output_index);
            self.reasoning.item_id = item_id.clone();
            self.reasoning.added = true;

            push_sse(
                output,
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": {
                        "id": item_id,
                        "type": "reasoning",
                        "status": "in_progress",
                        "reasoning_content": "",
                        "summary": []
                    }
                }),
            );
            push_sse(
                output,
                "response.reasoning_summary_part.added",
                json!({
                    "type": "response.reasoning_summary_part.added",
                    "item_id": self.reasoning.item_id,
                    "output_index": output_index,
                    "summary_index": 0,
                    "part": { "type": "summary_text", "text": "" }
                }),
            );
        }

        self.reasoning.text.push_str(delta);
        let output_index = self.reasoning.output_index.unwrap_or(0);
        push_sse(
            output,
            "response.reasoning_summary_text.delta",
            json!({
                "type": "response.reasoning_summary_text.delta",
                "item_id": self.reasoning.item_id,
                "output_index": output_index,
                "summary_index": 0,
                "delta": delta
            }),
        );
    }

    fn push_text_delta_into(&mut self, delta: &str, output: &mut String) {
        if !self.text.added {
            let output_index = self.next_output_index();
            let item_id = format!("msg_{}", response_id_body(&self.response_id));
            self.text.output_index = Some(output_index);
            self.text.item_id = item_id.clone();
            self.text.added = true;
            push_sse(
                output,
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": {
                        "id": item_id,
                        "type": "message",
                        "status": "in_progress",
                        "role": "assistant",
                        "content": []
                    }
                }),
            );
            push_sse(
                output,
                "response.content_part.added",
                json!({
                    "type": "response.content_part.added",
                    "item_id": self.text.item_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "part": { "type": "output_text", "text": "", "annotations": [] }
                }),
            );
        }

        self.text.text.push_str(delta);
        let output_index = self.text.output_index.unwrap_or(0);
        push_sse(
            output,
            "response.output_text.delta",
            json!({
                "type": "response.output_text.delta",
                "item_id": self.text.item_id,
                "output_index": output_index,
                "content_index": 0,
                "delta": delta
            }),
        );
    }

    fn push_tool_call_delta_into(&mut self, tool_call: &Value, output: &mut String) {
        let chat_index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let id_delta = tool_call
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let function = tool_call.get("function").unwrap_or(&Value::Null);
        let name_delta = function
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string);
        let args_delta = function
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let mut should_add = false;
        let mut output_index = None;
        let mut item_id = String::new();
        let mut pending_arguments = String::new();

        {
            let state = self.tools.entry(chat_index).or_default();
            if let Some(id) = id_delta {
                state.call_id = id;
            }
            if let Some(name) = name_delta {
                if !name.is_empty() {
                    state.name = name;
                }
            }
            if !args_delta.is_empty() {
                state.arguments.push_str(&args_delta);
            }

            // Custom tool output items must use the `ctc_` ID namespace. Some
            // Chat Completions providers send the call ID before the function
            // name, so wait for the name when the request includes custom tools
            // instead of emitting a provisional `function_call` with an `fc_`
            // ID that cannot later be replayed as a `custom_tool_call`.
            let waiting_for_custom_tool_name =
                self.tool_context.has_custom_tools && state.name.is_empty();
            if !state.added
                && (!state.call_id.is_empty() || !state.name.is_empty())
                && !waiting_for_custom_tool_name
            {
                should_add = true;
                pending_arguments = state.arguments.clone();
            } else if state.added {
                output_index = state.output_index;
                item_id = state.item_id.clone();
            }
        }

        if should_add {
            let assigned = self.next_output_index();
            let state = self.tools.get_mut(&chat_index).expect("tool state exists");
            state.added = true;
            if state.call_id.is_empty() {
                state.call_id = format!("call_{chat_index}");
            }
            if state.name.is_empty() {
                state.name = "unknown_tool".to_string();
            }
            state.output_index = Some(assigned);
            state.item_id = tool_call_item_id(&state.call_id, &state.name, &self.tool_context);
            let added_item = tool_call_added_item(state, assigned, &self.tool_context);
            push_sse(output, "response.output_item.added", added_item);
            if !pending_arguments.is_empty() {
                push_tool_call_delta_sse(
                    output,
                    state,
                    assigned,
                    &pending_arguments,
                    &self.tool_context,
                );
            }
        } else if !args_delta.is_empty() {
            if let Some(output_index) = output_index {
                let state = ToolCallState {
                    output_index: Some(output_index),
                    item_id,
                    name: self
                        .tools
                        .get(&chat_index)
                        .map(|state| state.name.clone())
                        .unwrap_or_default(),
                    call_id: self
                        .tools
                        .get(&chat_index)
                        .map(|state| state.call_id.clone())
                        .unwrap_or_default(),
                    ..ToolCallState::default()
                };
                push_tool_call_delta_sse(
                    output,
                    &state,
                    output_index,
                    &args_delta,
                    &self.tool_context,
                );
            }
        }
    }

    fn finalize_into(&mut self, output: &mut String) {
        if self.completed {
            return;
        }
        self.ensure_response_started_into(output);
        self.flush_inline_think_at_boundary_into(output);
        self.finalize_reasoning_into(output);
        self.finalize_text_into(output);
        self.finalize_tools_into(output);

        let status = response_status(self.finish_reason.as_deref());
        let mut response = self.base_response(status, self.completed_output_items());
        if status == "incomplete" {
            response["incomplete_details"] = json!({ "reason": "max_output_tokens" });
        }
        copy_response_request_fields(&mut response, self.original_request.as_ref());
        push_sse(
            output,
            "response.completed",
            json!({
                "type": "response.completed",
                "response": response
            }),
        );
        output.push_str("data: [DONE]\n\n");
        self.completed = true;
    }

    fn finalize_reasoning_into(&mut self, output: &mut String) {
        if !self.reasoning.added || self.reasoning.done {
            return;
        }
        let output_index = self.reasoning.output_index.unwrap_or(0);
        let item = json!({
            "id": self.reasoning.item_id,
            "type": "reasoning",
            "reasoning_content": self.reasoning.text,
            "summary": [{ "type": "summary_text", "text": self.reasoning.text }]
        });
        self.output_items.push((output_index, item.clone()));
        self.reasoning.done = true;
        push_sse(
            output,
            "response.reasoning_summary_text.done",
            json!({
                "type": "response.reasoning_summary_text.done",
                "item_id": self.reasoning.item_id,
                "output_index": output_index,
                "summary_index": 0,
                "text": self.reasoning.text
            }),
        );
        push_sse(
            output,
            "response.reasoning_summary_part.done",
            json!({
                "type": "response.reasoning_summary_part.done",
                "item_id": self.reasoning.item_id,
                "output_index": output_index,
                "summary_index": 0,
                "part": { "type": "summary_text", "text": self.reasoning.text }
            }),
        );
        push_sse(
            output,
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": item
            }),
        );
    }

    fn finalize_text_into(&mut self, output: &mut String) {
        if !self.text.added || self.text.done {
            return;
        }
        let output_index = self.text.output_index.unwrap_or(0);
        let item = json!({
            "id": self.text.item_id,
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": self.text.text, "annotations": [] }]
        });
        self.output_items.push((output_index, item.clone()));
        self.text.done = true;
        push_sse(
            output,
            "response.output_text.done",
            json!({
                "type": "response.output_text.done",
                "item_id": self.text.item_id,
                "output_index": output_index,
                "content_index": 0,
                "text": self.text.text
            }),
        );
        push_sse(
            output,
            "response.content_part.done",
            json!({
                "type": "response.content_part.done",
                "item_id": self.text.item_id,
                "output_index": output_index,
                "content_index": 0,
                "part": { "type": "output_text", "text": self.text.text, "annotations": [] }
            }),
        );
        push_sse(
            output,
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": item
            }),
        );
    }

    fn finalize_tools_into(&mut self, output: &mut String) {
        let keys: Vec<usize> = self.tools.keys().copied().collect();
        for key in keys {
            if self.tools.get(&key).map(|state| state.done).unwrap_or(true) {
                continue;
            }
            if self
                .tools
                .get(&key)
                .map(|state| !state.added && !state.done)
                .unwrap_or(false)
            {
                let assigned = self.next_output_index();
                let state = self.tools.get_mut(&key).expect("tool state exists");
                state.added = true;
                if state.call_id.is_empty() {
                    state.call_id = format!("call_{key}");
                }
                if state.name.is_empty() {
                    state.name = "unknown_tool".to_string();
                }
                state.output_index = Some(assigned);
                state.item_id = tool_call_item_id(&state.call_id, &state.name, &self.tool_context);
                let added_item = tool_call_added_item(state, assigned, &self.tool_context);
                push_sse(output, "response.output_item.added", added_item);
            }

            let state = self.tools.get_mut(&key).expect("tool state exists");
            let output_index = state.output_index.unwrap_or(0);
            let item = tool_call_done_item(state, &self.tool_context);
            state.done = true;
            self.output_items.push((output_index, item.clone()));
            push_tool_call_done_sse(output, state, output_index, &self.tool_context);
            push_sse(
                output,
                "response.output_item.done",
                json!({
                    "type": "response.output_item.done",
                    "output_index": output_index,
                    "item": item
                }),
            );
        }
    }

    fn failed_into(&mut self, output: &mut String, message: String, error_type: Option<String>) {
        self.completed = true;
        let mut error = json!({ "message": message });
        if let Some(error_type) = error_type.filter(|value| !value.is_empty()) {
            error["type"] = json!(error_type);
        }
        let mut response = self.base_response("failed", self.completed_output_items());
        response["error"] = error;
        push_sse(
            output,
            "response.failed",
            json!({
                "type": "response.failed",
                "response": response
            }),
        );
    }

    fn completed_output_items(&self) -> Vec<Value> {
        let mut output_items = self.output_items.clone();
        output_items.sort_by_key(|(output_index, _)| *output_index);
        output_items.into_iter().map(|(_, item)| item).collect()
    }

    fn base_response(&self, status: &str, output: Vec<Value>) -> Value {
        json!({
            "id": self.response_id,
            "object": "response",
            "created_at": self.created_at,
            "status": status,
            "model": self.model,
            "output": output,
            "usage": self.latest_usage.clone().unwrap_or_else(default_responses_usage)
        })
    }

    fn next_output_index(&mut self) -> u32 {
        let index = self.next_output_index;
        self.next_output_index += 1;
        index
    }
}

fn take_sse_block(buffer: &mut String) -> Option<String> {
    let lf = buffer.find("\n\n").map(|index| (index, 2));
    let crlf = buffer.find("\r\n\r\n").map(|index| (index, 4));
    let (index, delimiter_len) = match (lf, crlf) {
        (Some(left), Some(right)) => {
            if left.0 <= right.0 {
                left
            } else {
                right
            }
        }
        (Some(value), None) | (None, Some(value)) => value,
        (None, None) => return None,
    };
    let block = buffer[..index].to_string();
    buffer.drain(..index + delimiter_len);
    Some(block)
}

fn append_utf8_safe(buffer: &mut String, remainder: &mut Vec<u8>, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let mut combined = Vec::new();
    if !remainder.is_empty() {
        combined.extend_from_slice(remainder);
        remainder.clear();
    }
    combined.extend_from_slice(bytes);

    match std::str::from_utf8(&combined) {
        Ok(text) => buffer.push_str(text),
        Err(error) => {
            let valid = error.valid_up_to();
            if valid > 0 {
                buffer.push_str(std::str::from_utf8(&combined[..valid]).unwrap_or_default());
            }
            if error.error_len().is_none() {
                remainder.extend_from_slice(&combined[valid..]);
            } else {
                buffer.push_str(&String::from_utf8_lossy(&combined[valid..]));
            }
        }
    }
}

fn strip_sse_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(field)?.strip_prefix(':')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

fn chat_delta_reasoning_text(delta: &Value) -> Option<String> {
    extract_reasoning_field_text(delta)
}

enum ThinkPrefixDecision {
    NeedMore,
    Reasoning,
    Text,
}

fn leading_think_prefix_decision(buffer: &str) -> ThinkPrefixDecision {
    let trimmed = buffer.trim_start();
    if trimmed.is_empty() {
        return ThinkPrefixDecision::NeedMore;
    }
    if trimmed.starts_with(THINK_OPEN_TAG) {
        return ThinkPrefixDecision::Reasoning;
    }
    if THINK_OPEN_TAG.starts_with(trimmed) {
        return ThinkPrefixDecision::NeedMore;
    }
    ThinkPrefixDecision::Text
}

fn extract_chat_sse_error(value: &Value) -> (String, Option<String>) {
    let error = value.get("error").unwrap_or(value);
    let message = error
        .as_str()
        .map(ToString::to_string)
        .or_else(|| {
            error
                .get("message")
                .or_else(|| error.get("detail"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| error.to_string());
    let error_type = error
        .get("type")
        .or_else(|| error.get("code"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (message, error_type)
}

fn http_status_line(status: u16) -> String {
    match status {
        200 => "200 OK".to_string(),
        400 => "400 Bad Request".to_string(),
        401 => "401 Unauthorized".to_string(),
        403 => "403 Forbidden".to_string(),
        404 => "404 Not Found".to_string(),
        429 => "429 Too Many Requests".to_string(),
        500 => "500 Internal Server Error".to_string(),
        502 => "502 Bad Gateway".to_string(),
        503 => "503 Service Unavailable".to_string(),
        _ => format!("{status} Upstream"),
    }
}

pub fn responses_error_from_upstream(status_code: u16, content_type: &str, body: &[u8]) -> Value {
    let (message, error_type, code, param) = upstream_error_parts(status_code, content_type, body);
    let mut error = json!({
        "message": message,
        "type": error_type.unwrap_or_else(|| "upstream_error".to_string()),
    });
    if let Some(code) = code {
        error["code"] = json!(code);
    }
    if let Some(param) = param {
        error["param"] = json!(param);
    }
    json!({ "error": error })
}

fn upstream_error_parts(
    status_code: u16,
    content_type: &str,
    body: &[u8],
) -> (String, Option<String>, Option<String>, Option<String>) {
    if content_type.to_ascii_lowercase().contains("json") {
        if let Ok(value) = serde_json::from_slice::<Value>(body) {
            let error = value.get("error").unwrap_or(&value);
            let message = error
                .get("message")
                .or_else(|| error.get("detail"))
                .or_else(|| error.get("error"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| truncate_error_preview(&value.to_string()));
            let error_type = error
                .get("type")
                .or_else(|| error.get("error_type"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let code = error.get("code").and_then(|value| {
                value
                    .as_str()
                    .map(ToString::to_string)
                    .or_else(|| value.as_i64().map(|number| number.to_string()))
            });
            let param = error
                .get("param")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            return (message, error_type, code, param);
        }
    }

    let preview = truncate_error_preview(&String::from_utf8_lossy(body));
    let message = if preview.trim().is_empty() {
        format!("Upstream returned HTTP {status_code}")
    } else {
        preview
    };
    (message, None, Some(status_code.to_string()), None)
}

fn truncate_error_preview(input: &str) -> String {
    input.chars().take(ERROR_BODY_PREVIEW_LIMIT).collect()
}

/// Responses 协议要求每个 input item 的 `id` 前缀与它的 `type` 对应，
/// 例如 `message` 必须是 `msg_`、`function_call` 必须是 `fc_`。
/// 前缀不匹配时上游直接以 `[ApiIdParam] [input[N].id] [invalid_id_prefix]` 拒绝**整份**请求，
/// 于是这段历史被反复重放、会话永久不可用（#1431 / #1781 / #1796）。
///
/// 前缀表取自 Codex 自己的 rollout 记录（`~/.codex/sessions/**/rollout-*.jsonl`），
/// 不是猜测：`message`/`reasoning`/`custom_tool_call`/`custom_tool_call_output`/
/// `function_call`/`function_call_output` 分别对应 msg_/rs_/ctc_/ctco_/fc_/fco_。
const RESPONSES_ITEM_ID_PREFIXES: &[(&str, &str)] = &[
    ("message", "msg_"),
    ("reasoning", "rs_"),
    ("compaction", "cmp_"),
    ("function_call", "fc_"),
    ("function_call_output", "fco_"),
    ("custom_tool_call", "ctc_"),
    ("custom_tool_call_output", "ctco_"),
];

/// 供集成测试直接验证前缀归一结果：`upstream_request_parts` 需要真实网络，
/// 用它测不方便。
#[doc(hidden)]
pub fn normalize_responses_item_ids_for_test(body: &mut Value) {
    normalize_responses_item_ids(body);
}

/// 出站前把所有已知 item 的 id 前缀修正到与 `type` 一致。
///
/// 只认前缀表里的类型，未知类型原样通过——宁可放过，也不要把看不出类型语义的 id 改坏。
fn normalize_responses_item_ids(body: &mut Value) {
    let Some(input) = body.get_mut("input") else {
        return;
    };
    match input {
        Value::Array(items) => {
            for item in items {
                normalize_responses_item_id(item);
            }
        }
        Value::Object(_) => normalize_responses_item_id(input),
        _ => {}
    }
}

fn normalize_responses_item_id(item: &mut Value) {
    let Some(item_type) = item.get("type").and_then(Value::as_str) else {
        return;
    };
    let Some((_, prefix)) = RESPONSES_ITEM_ID_PREFIXES
        .iter()
        .find(|(kind, _)| *kind == item_type)
    else {
        return;
    };
    let Some(id) = item.get("id").and_then(Value::as_str) else {
        return;
    };
    // 只有「前缀 + 非空后缀」才算已经合规。id 恰好等于前缀本身（`fc_`）是空壳，
    // 放它过去会直接触发上游的 invalid_id_prefix，所以落到下面按 call_id 重建。
    if id.len() > prefix.len() && id.starts_with(prefix) {
        return;
    }
    // 剥掉 id 上现有的前缀再换新的。取**最长**匹配：`fc_` 是 `fco_` 的前缀，
    // 先撞上短的会把 `fco_call_a` 剥成 `call_a` 再换成 `fc_call_a`，
    // 把 function_call_output 错改成 function_call。
    // `item_` 是历史版本 Codex++ 自己造的前缀；`cp_` 来自压缩项错误使用
    // `cp_{response_id}` 的版本；`resp_` 来自历史版本把 message item 命名成
    // `{response_id}_msg`（#1431 / #1781），都要一并剥掉。
    let suffix = ["item_", "cp_", "resp_"]
        .into_iter()
        .chain(RESPONSES_ITEM_ID_PREFIXES.iter().map(|(_, known)| *known))
        .filter_map(|known| id.strip_prefix(known).map(|rest| (known.len(), rest)))
        .max_by_key(|(len, _)| *len)
        .map(|(_, rest)| rest)
        .unwrap_or(id);
    // 剥完是空串说明 id 恰好只由某个前缀组成，退回 call_id，再退回原 id。
    let suffix = if suffix.is_empty() {
        item.get("call_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(id)
    } else {
        suffix
    };
    item["id"] = json!(format!("{prefix}{suffix}"));
}

fn append_responses_input(input: &Value, messages: &mut Vec<Value>) {
    match input {
        Value::String(text) => messages.push(json!({ "role": "user", "content": text })),
        Value::Array(items) => {
            let mut pending_tool_calls = Vec::new();
            let mut pending_reasoning = Vec::new();
            let mut seen_tool_call_ids = BTreeSet::new();
            for item in items {
                append_responses_item(
                    item,
                    messages,
                    &mut pending_tool_calls,
                    &mut pending_reasoning,
                    &mut seen_tool_call_ids,
                );
            }
            flush_tool_calls(messages, &mut pending_tool_calls, &mut pending_reasoning);
            flush_reasoning(messages, &mut pending_reasoning);
        }
        Value::Object(_) => {
            let mut pending_tool_calls = Vec::new();
            let mut pending_reasoning = Vec::new();
            let mut seen_tool_call_ids = BTreeSet::new();
            append_responses_item(
                input,
                messages,
                &mut pending_tool_calls,
                &mut pending_reasoning,
                &mut seen_tool_call_ids,
            );
            flush_tool_calls(messages, &mut pending_tool_calls, &mut pending_reasoning);
            flush_reasoning(messages, &mut pending_reasoning);
        }
        _ => {}
    }
}

fn append_responses_item(
    item: &Value,
    messages: &mut Vec<Value>,
    pending_tool_calls: &mut Vec<Value>,
    pending_reasoning: &mut Vec<String>,
    seen_tool_call_ids: &mut BTreeSet<String>,
) {
    match item.get("type").and_then(Value::as_str) {
        Some("function_call") => {
            let name = responses_history_function_name(item);
            if name.is_empty() {
                return;
            }
            let call_id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if call_id.is_empty() {
                return;
            }
            seen_tool_call_ids.insert(call_id.to_string());
            pending_tool_calls.push(json!({
                "id": call_id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": responses_arguments_to_chat(item.get("arguments").unwrap_or(&json!({})))
                }
            }));
        }
        Some("function_call_output") => {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
            if call_id.is_empty() {
                return;
            }
            if !seen_tool_call_ids.contains(call_id) {
                flush_tool_calls(messages, pending_tool_calls, pending_reasoning);
                flush_reasoning(messages, pending_reasoning);
                messages.push(orphan_tool_output_message(
                    call_id,
                    item.get("output").unwrap_or(&Value::Null),
                ));
                return;
            }
            flush_tool_calls(messages, pending_tool_calls, pending_reasoning);
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": tool_output_content(item.get("output").unwrap_or(&Value::Null))
            }));
        }
        Some("custom_tool_call") => {
            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
            let input = item
                .get("input")
                .or_else(|| item.get("arguments"))
                .unwrap_or(&Value::Null);
            let (name, arguments) = build_custom_tool_call_history(name, input);
            let call_id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if call_id.is_empty() {
                return;
            }
            seen_tool_call_ids.insert(call_id.to_string());
            pending_tool_calls.push(json!({
                "id": call_id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": arguments
                }
            }));
        }
        Some("tool_search_call") => {
            // Codex 的 tool_search 是客户端执行的代理工具，历史回放时按
            // function tool_call 形态映射进 chat 消息流。
            let call_id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if call_id.is_empty() {
                return;
            }
            seen_tool_call_ids.insert(call_id.to_string());
            pending_tool_calls.push(json!({
                "id": call_id,
                "type": "function",
                "function": {
                    "name": "tool_search",
                    "arguments": responses_arguments_to_chat(
                        item.get("arguments").unwrap_or(&json!({}))
                    )
                }
            }));
        }
        Some("tool_search_output") => {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
            if call_id.is_empty() {
                return;
            }
            let output = item.get("tools").unwrap_or(&Value::Null);
            if !seen_tool_call_ids.contains(call_id) {
                flush_tool_calls(messages, pending_tool_calls, pending_reasoning);
                flush_reasoning(messages, pending_reasoning);
                messages.push(orphan_tool_output_message(call_id, output));
                return;
            }
            flush_tool_calls(messages, pending_tool_calls, pending_reasoning);
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": tool_output_content(output)
            }));
        }
        Some("custom_tool_call_output") => {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
            if call_id.is_empty() {
                return;
            }
            if !seen_tool_call_ids.contains(call_id) {
                flush_tool_calls(messages, pending_tool_calls, pending_reasoning);
                flush_reasoning(messages, pending_reasoning);
                messages.push(orphan_tool_output_message(
                    call_id,
                    item.get("output").unwrap_or(&Value::Null),
                ));
                return;
            }
            flush_tool_calls(messages, pending_tool_calls, pending_reasoning);
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": tool_output_content(item.get("output").unwrap_or(&Value::Null))
            }));
        }
        Some("tool_call") => {
            if let Some(tool_use) = item.get("tool_use") {
                let call_id = tool_use
                    .get("id")
                    .or_else(|| item.get("call_id"))
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if call_id.is_empty() {
                    return;
                }
                seen_tool_call_ids.insert(call_id.to_string());
                pending_tool_calls.push(json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": tool_use.get("name").and_then(Value::as_str).unwrap_or(""),
                        "arguments": responses_arguments_to_chat(tool_use.get("input").unwrap_or(&json!({})))
                    }
                }));
            }
        }
        Some("tool_result") => {
            flush_tool_calls(messages, pending_tool_calls, pending_reasoning);
            let content = item.get("content").unwrap_or(&Value::Null);
            let call_id = content
                .get("tool_use_id")
                .or_else(|| item.get("tool_call_id"))
                .or_else(|| item.get("call_id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if call_id.is_empty() {
                return;
            }
            let output = content.get("content").unwrap_or(content);
            if !seen_tool_call_ids.contains(call_id) {
                flush_reasoning(messages, pending_reasoning);
                messages.push(orphan_tool_output_message(call_id, output));
                return;
            }
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": tool_output_content(output)
            }));
        }
        Some("reasoning") => {
            if let Some(text) = responses_reasoning_text(item) {
                if !text.is_empty() {
                    pending_reasoning.push(text);
                }
            }
        }
        Some(COMPACTION_OUTPUT_TYPE) => {
            // codex 历史回放：上次压缩的结果以 `compaction` item 形式出现在 input
            // 里，`encrypted_content` 是我们生成的明文摘要，展开成 user 消息喂给上游。
            flush_tool_calls(messages, pending_tool_calls, pending_reasoning);
            flush_reasoning(messages, pending_reasoning);
            if let Some(message) = expand_compaction_item(item) {
                messages.push(message);
            }
        }
        Some(COMPACTION_TRIGGER_TYPE) => {
            // 控制项不进上游历史；正常请求不该出现，出现即忽略。
        }
        _ => {
            flush_tool_calls(messages, pending_tool_calls, pending_reasoning);
            if let Some(content) = item.get("content") {
                let role = responses_role_to_chat_role(item.get("role").and_then(Value::as_str));
                if content.is_null() && role != "assistant" {
                    return;
                }
                let mut message = json!({
                    "role": role,
                    "content": responses_content_to_chat_content(role, content)
                });
                if role == "assistant" {
                    if !pending_reasoning.is_empty() && pending_tool_calls.is_empty() {
                        message["reasoning_content"] =
                            json!(std::mem::take(pending_reasoning).join("\n"));
                    }
                } else if !pending_reasoning.is_empty() {
                    flush_tool_calls(messages, pending_tool_calls, pending_reasoning);
                    flush_reasoning(messages, pending_reasoning);
                }
                messages.push(message);
            }
        }
    }
}

fn orphan_tool_output_message(call_id: &str, output: &Value) -> Value {
    // 这条已经是 user 消息，multi-part 图片可以直接内联，不必走 relocate。
    if let Value::Array(parts) = tool_output_content(output) {
        let mut content = vec![json!({
            "type": "text",
            "text": format!("Function call output ({call_id}):")
        })];
        content.extend(parts);
        return json!({ "role": "user", "content": content });
    }
    json!({
        "role": "user",
        "content": format!(
            "Function call output ({call_id}): {}",
            response_output_text(output)
        )
    })
}

/// Chat Completions 上游（DeepSeek thinking 模式尤其严格）要求带 `tool_calls` 的
/// assistant 消息后面必须紧跟每个 `tool_call_id` 对应的 `tool` 消息。中断/回滚过的
/// 一轮会话可能留下没有 output 的 `function_call`，直接转发会被上游 400
/// （insufficient tool messages following tool_calls message）。
///
/// 这里把没有配对 output 的 tool_call 从消息里摘掉，降级成文本保留在历史中，
/// 避免丢失「模型曾试图调用某工具」这一信息。
/// 把插在「连续 tool 结果」之间的非 tool 消息整体搬到该 tool 区之后
/// （issue #2275 / #2257）。
///
/// 上游 codex 会把 `<image_resize_notice>` 这类提示以 developer（映射成 system）
/// 或 user 消息的形式插在两条 tool 结果之间，形成夹心结构：
/// `assistant(tool_calls=[a,b]) → tool(a) → developer → tool(b)`。
/// `enforce_tool_call_pairing` 用 `take_while` 只收集「role 连续为 tool」的后续消息，
/// 数到夹心就停，于是 followers 只有 1 条、`b` 被误判 orphaned 并从 `tool_calls`
/// 摘掉，但 `b` 的 tool 消息还留在原地 —— 这正是「role 'tool' 无前置 tool_calls」
/// 与「No tool output found for tool call」的来源。
///
/// 这里在配对判定**之前**把夹心消息移到 tool 区之后，使 tool 结果重新连续。
/// system 形态的夹心会被后续的 `collapse_system_messages_to_head` 带到头部（合法），
/// user 形态的则留在 tool 区之后（同样合法）。
fn relocate_interleaved_non_tool_messages(messages: &mut Vec<Value>) {
    let mut index = 0;
    while index < messages.len() {
        let Some(tool_calls) = messages[index].get("tool_calls").and_then(Value::as_array) else {
            index += 1;
            continue;
        };
        let mut unanswered: BTreeSet<String> = tool_calls
            .iter()
            .filter_map(|tool_call| tool_call.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        if unanswered.is_empty() {
            index += 1;
            continue;
        }

        // 从本条 assistant 往后扫，收集本轮的所有 tool 结果与夹在其中的非 tool 消息，
        // 直到 tool_call 集合配齐、撞上下一条 assistant（新轮次，不能越界）、或到底。
        let mut tool_messages: Vec<Value> = Vec::new();
        let mut interleaved: Vec<Value> = Vec::new();
        let mut scan = index + 1;
        while scan < messages.len() {
            let role = messages[scan].get("role").and_then(Value::as_str);
            if role == Some("assistant") {
                break;
            }
            if role == Some("tool") {
                if let Some(id) = messages[scan].get("tool_call_id").and_then(Value::as_str) {
                    unanswered.remove(id);
                }
                tool_messages.push(messages[scan].clone());
            } else if !tool_messages.is_empty() {
                // 只有已经收到过 tool 结果之后的夹心才值得搬：
                // 本条 assistant 尚未收到任何结果时，中间的消息是正常历史，不是夹心。
                interleaved.push(messages[scan].clone());
            }
            scan += 1;
            if unanswered.is_empty() {
                break;
            }
        }

        if interleaved.is_empty() {
            index += 1;
            continue;
        }

        // 重建这段区间：tool 结果。配齐时）在前、夹心消息在后。
        let rebuilt: Vec<Value> = tool_messages.into_iter().chain(interleaved).collect();
        let rebuilt_len = rebuilt.len();
        messages.splice(index + 1..scan, rebuilt);
        // 跳过刚重建的区间，避免在搬动过的消息上重复扫描导致死循环。
        index += 1 + rebuilt_len;
    }
}

/// 把没有前置 `tool_calls` 的 tool 消息降级成 user（issue #2275 / #2257 的防线）。
///
/// `enforce_tool_call_pairing` 目前只清理 assistant 侧（把 orphaned 的 tool_call
/// 从 `tool_calls` 摘掉），被摘掉的那些 tool 消息本身仍留在原位，上游会直接
/// 400「No tool output found for tool call」。这里把它们降级成 user，保住内容
/// 且不再触发协议错误。
fn degrade_unpaired_tool_messages(messages: &mut [Value]) {
    let mut known: BTreeSet<String> = BTreeSet::new();
    for message in messages.iter() {
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            for tool_call in tool_calls {
                if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                    known.insert(id.to_string());
                }
            }
        }
    }

    for message in messages.iter_mut() {
        if message.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let paired = message
            .get("tool_call_id")
            .and_then(Value::as_str)
            .is_some_and(|id| known.contains(id));
        if paired {
            continue;
        }
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        *message = json!({
            "role": "user",
            "content": content
        });
    }
}

fn enforce_tool_call_pairing(messages: &mut [Value]) {
    let mut index = 0;
    while index < messages.len() {
        if messages[index].get("role").and_then(Value::as_str) != Some("assistant") {
            index += 1;
            continue;
        }
        let Some(tool_calls) = messages[index].get("tool_calls").and_then(Value::as_array) else {
            index += 1;
            continue;
        };
        if tool_calls.is_empty() {
            index += 1;
            continue;
        }

        // 收集紧跟其后的 tool 消息所应答的 id
        let mut answered = BTreeSet::new();
        let mut followers = 0;
        for message in messages[index + 1..]
            .iter()
            .take_while(|message| message.get("role").and_then(Value::as_str) == Some("tool"))
        {
            followers += 1;
            if let Some(id) = message.get("tool_call_id").and_then(Value::as_str) {
                answered.insert(id.to_string());
            }
        }

        // 位于历史尾部的 tool_call 是「刚发起、output 还没回来」的正常形态，
        // 上游本就期待它；只有序列越过了它却没应答才是非法的。
        if index + 1 + followers >= messages.len() {
            index += 1;
            continue;
        }

        let (kept, orphaned): (Vec<Value>, Vec<Value>) =
            tool_calls.iter().cloned().partition(|tool_call| {
                tool_call
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| answered.contains(id))
            });
        if orphaned.is_empty() {
            index += 1;
            continue;
        }

        let notes = orphaned
            .iter()
            .map(|tool_call| {
                let name = tool_call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let id = tool_call.get("id").and_then(Value::as_str).unwrap_or("");
                format!("Abandoned function call ({id}): {name}")
            })
            .collect::<Vec<_>>()
            .join("\n");

        if kept.is_empty() {
            if let Some(message) = messages[index].as_object_mut() {
                message.remove("tool_calls");
            }
        } else {
            messages[index]["tool_calls"] = json!(kept);
        }
        append_text_to_assistant_message(&mut messages[index], &notes);
        index += 1;
    }
}

/// `role:"tool"` 消息在多数 Chat Completions 上游（DeepSeek 在内）只接受字符串 content，
/// 塞 multi-part `image_url` 会被 400；而 `enforce_tool_call_pairing` 又要求 tool 消息
/// 紧跟在 assistant(tool_calls) 之后**连续**排列 —— 把图片消息插在两条 tool 消息中间，
/// 会让后面那条不再被计入 followers，对应的 tool_call 被误判成 orphaned 而摘掉。
///
/// 两个约束都要满足，所以这里的做法是：tool 消息本身降级成文本占位，图片一路收集到
/// 连续 tool 区结束，再作为一条 user 消息整体插在其后。
fn relocate_tool_output_images(messages: &mut Vec<Value>) {
    let mut index = 0;
    while index < messages.len() {
        if messages[index].get("role").and_then(Value::as_str) != Some("tool") {
            index += 1;
            continue;
        }
        let mut images = Vec::new();
        let mut end = index;
        while end < messages.len()
            && messages[end].get("role").and_then(Value::as_str) == Some("tool")
        {
            take_images_from_tool_message(&mut messages[end], &mut images);
            end += 1;
        }
        if !images.is_empty() {
            let mut content = vec![json!({
                "type": "text",
                "text": "Images returned by the tool call(s) above:"
            })];
            content.append(&mut images);
            messages.insert(end, json!({ "role": "user", "content": content }));
            end += 1;
        }
        index = end;
    }
}

/// 摘掉单条 tool 消息里的图片块，content 降级为纯文本。
/// 没有文本时补占位符 —— 空字符串 content 会被部分上游拒绝。
fn take_images_from_tool_message(message: &mut Value, images: &mut Vec<Value>) {
    let Some(parts) = message.get("content").and_then(Value::as_array) else {
        return;
    };

    let mut texts = Vec::new();
    let mut found = Vec::new();
    for part in parts {
        if is_image_part(part) {
            if let Some(image) = image_part_to_chat(part) {
                found.push(image);
            }
            continue;
        }
        if let Some(text) = part.get("text").and_then(Value::as_str)
            && !text.is_empty()
        {
            texts.push(text.to_string());
        }
    }
    if found.is_empty() {
        return;
    }

    let placeholder = if found.len() == 1 {
        "[image]".to_string()
    } else {
        format!("[{} images]", found.len())
    };
    message["content"] = json!(if texts.is_empty() {
        placeholder
    } else {
        format!("{}\n{placeholder}", texts.join("\n"))
    });
    images.append(&mut found);
}

fn append_text_to_assistant_message(message: &mut Value, text: &str) {
    if text.is_empty() {
        return;
    }
    let existing = match message.get("content") {
        Some(Value::String(content)) => content.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| part.as_str())
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };
    message["content"] = if existing.trim().is_empty() {
        json!(text)
    } else {
        json!(format!("{existing}\n{text}"))
    };
}

/// DeepSeek thinking 模式要求带 `tool_calls` 的 assistant 消息回传 `reasoning_content`，
/// 否则报 400（The `reasoning_content` in the thinking mode must be passed back to the API）。
/// 历史里没有 reasoning 项时（例如上游没回传 summary，或被裁剪掉了）补一个占位说明，
/// 只补 content 和 reasoning_content 同时为空的情况，不覆盖真实 reasoning。
fn ensure_tool_call_reasoning_content(messages: &mut [Value]) {
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let has_tool_calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|tool_calls| !tool_calls.is_empty());
        if !has_tool_calls {
            continue;
        }
        let has_content = message
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|content| !content.trim().is_empty());
        let has_reasoning = message
            .get("reasoning_content")
            .and_then(Value::as_str)
            .is_some_and(|reasoning| !reasoning.trim().is_empty());
        if has_content || has_reasoning {
            continue;
        }
        message["reasoning_content"] = json!("Calling the requested tool.");
    }
}

fn normalize_chat_messages(messages: &mut [Value]) {
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let has_content = match message.get("content") {
            Some(Value::Null) | None => false,
            Some(Value::String(_)) => true,
            Some(Value::Array(parts)) => !parts.is_empty(),
            Some(_) => true,
        };
        let has_tool_calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|tool_calls| !tool_calls.is_empty());
        if !has_content && !has_tool_calls {
            message["content"] = json!("");
        }
    }
}

fn collapse_system_messages_to_head(messages: Vec<Value>) -> Vec<Value> {
    let mut system_chunks = Vec::new();
    let mut rest = Vec::with_capacity(messages.len());

    for message in messages {
        if message.get("role").and_then(Value::as_str) == Some("system") {
            if let Some(text) = message.get("content").and_then(Value::as_str) {
                if !text.trim().is_empty() {
                    system_chunks.push(text.to_string());
                }
                continue;
            }
        }
        rest.push(message);
    }

    let mut output = Vec::with_capacity(rest.len() + usize::from(!system_chunks.is_empty()));
    if !system_chunks.is_empty() {
        output.push(json!({
            "role": "system",
            "content": system_chunks.join("\n\n")
        }));
    }
    output.extend(rest);
    output
}

fn responses_role_to_chat_role(role: Option<&str>) -> &'static str {
    match role {
        Some("developer") | Some("system") => "system",
        Some("assistant") => "assistant",
        Some("tool") => "tool",
        Some("latest_reminder") => "user",
        Some("user") | None => "user",
        Some(_) => "user",
    }
}

fn flush_tool_calls(
    messages: &mut Vec<Value>,
    pending_tool_calls: &mut Vec<Value>,
    pending_reasoning: &mut Vec<String>,
) {
    if pending_tool_calls.is_empty() {
        return;
    }

    if let Some(last) = messages.last_mut() {
        if last.get("role").and_then(Value::as_str) == Some("assistant") {
            merge_tool_calls_into_message(last, std::mem::take(pending_tool_calls));
            // 合并路径同样要消费 pending_reasoning（issue #2210）。
            // 触发时序：reasoning item → 不带 tool_calls 的 assistant 文本消息
            // （pending_tool_calls 为空，reasoning 被附加到该文本消息并 take）→
            // function_call。此时最后一条已是 assistant，走本分支提前 return，
            // 若这里不追加，随后的 reasoning 就随函数返回被静默丢弃；
            // 而 ensure_tool_call_reasoning_content 只补 content 与
            // reasoning_content 同时为空的占位，content 非空时补不上。
            if !pending_reasoning.is_empty() {
                append_reasoning_to_assistant_message(
                    last,
                    &std::mem::take(pending_reasoning).join("\n"),
                );
            }
            return;
        }
    }

    let mut message = json!({
        "role": "assistant",
        "content": "",
        "tool_calls": std::mem::take(pending_tool_calls)
    });
    if !pending_reasoning.is_empty() {
        message["reasoning_content"] = json!(std::mem::take(pending_reasoning).join("\n"));
    }
    messages.push(message);
}

fn flush_reasoning(messages: &mut Vec<Value>, pending_reasoning: &mut Vec<String>) {
    if pending_reasoning.is_empty() {
        return;
    }
    let reasoning = std::mem::take(pending_reasoning).join("\n");
    if let Some(last) = messages.last_mut() {
        if last.get("role").and_then(Value::as_str) == Some("assistant") {
            append_reasoning_to_assistant_message(last, &reasoning);
            return;
        }
    }
    messages.push(json!({
        "role": "assistant",
        "content": "",
        "reasoning_content": reasoning
    }));
}

fn append_reasoning_to_assistant_message(message: &mut Value, reasoning: &str) {
    if reasoning.is_empty() {
        return;
    }
    let existing = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .unwrap_or("");
    message["reasoning_content"] = if existing.is_empty() {
        json!(reasoning)
    } else {
        json!(format!("{existing}\n{reasoning}"))
    };
    if message.get("content").is_none() || message.get("content") == Some(&Value::Null) {
        message["content"] = json!("");
    }
}

fn merge_tool_calls_into_message(message: &mut Value, incoming: Vec<Value>) {
    let Some(object) = message.as_object_mut() else {
        return;
    };
    let existing = object
        .entry("tool_calls".to_string())
        .or_insert_with(|| json!([]));
    let Some(existing_array) = existing.as_array_mut() else {
        *existing = json!(incoming);
        return;
    };
    for tool_call in incoming {
        let id = tool_call.get("id").and_then(Value::as_str).unwrap_or("");
        if !id.is_empty()
            && existing_array
                .iter()
                .any(|item| item.get("id").and_then(Value::as_str) == Some(id))
        {
            continue;
        }
        existing_array.push(tool_call);
    }
    if message.get("content").is_none() || message.get("content") == Some(&Value::Null) {
        message["content"] = json!("");
    }
}

fn responses_reasoning_text(item: &Value) -> Option<String> {
    extract_reasoning_summary_text(item).or_else(|| extract_reasoning_field_text(item))
}

fn is_image_part(part: &Value) -> bool {
    matches!(
        part.get("type").and_then(Value::as_str),
        Some("input_image") | Some("image_url")
    )
}

/// 把 Responses 的 `input_image`（`image_url` 可能是裸字符串）与已经是 Chat 形态的
/// `image_url` 统一成 Chat Completions 的 `{"type":"image_url","image_url":{"url":…}}`。
///
/// 返回 `None` 表示这不是图片块、或 url 为空不值得转发。
fn image_part_to_chat(part: &Value) -> Option<Value> {
    if !is_image_part(part) {
        return None;
    }
    let raw = part.get("image_url")?;
    let image_url = if raw.is_object() {
        raw.clone()
    } else {
        json!({ "url": raw.as_str().unwrap_or_default() })
    };
    if image_url
        .get("url")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return None;
    }
    Some(json!({ "type": "image_url", "image_url": image_url }))
}

fn responses_content_to_chat_content(_role: &str, content: &Value) -> Value {
    if content.is_null() || content.is_string() {
        return content.clone();
    }

    let Some(parts) = content.as_array() else {
        return content.clone();
    };
    let mut chat_parts = Vec::new();
    let mut has_non_text_part = false;

    for part in parts {
        match part.get("type").and_then(Value::as_str).unwrap_or("") {
            "input_text" | "output_text" | "text" => {
                if let Some(value) = part.get("text").and_then(Value::as_str) {
                    if !value.is_empty() {
                        chat_parts.push(json!({ "type": "text", "text": value }));
                    }
                }
            }
            "refusal" => {
                if let Some(value) = part.get("refusal").and_then(Value::as_str) {
                    if !value.is_empty() {
                        chat_parts.push(json!({ "type": "text", "text": value }));
                    }
                }
            }
            "input_image" | "image_url" => {
                if let Some(image) = image_part_to_chat(part) {
                    chat_parts.push(image);
                    has_non_text_part = true;
                }
            }
            _ => {}
        }
    }

    if !has_non_text_part {
        return Value::String(
            chat_parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    Value::Array(chat_parts)
}

fn responses_history_function_name(item: &Value) -> String {
    let name = item.get("name").and_then(Value::as_str).unwrap_or("");
    let namespace = item.get("namespace").and_then(Value::as_str).unwrap_or("");
    if name.is_empty() {
        String::new()
    } else if namespace.is_empty() {
        name.to_string()
    } else {
        flatten_namespace_tool_name(namespace, name)
    }
}

fn build_codex_tool_context(tools: Option<&Value>) -> CodexToolContext {
    let mut context = CodexToolContext::default();
    let Some(tools) = tools.and_then(Value::as_array) else {
        return context;
    };

    for tool in tools {
        if let Some(name) = tool.as_str().filter(|name| !name.is_empty()) {
            if let Some(action) = proxy_action_from_upstream_name(name) {
                context.custom_tools.insert(
                    name.to_string(),
                    CodexCustomToolSpec {
                        openai_name: "apply_patch".to_string(),
                        kind: CodexCustomToolKind::ApplyPatch,
                        proxy_action: Some(action),
                    },
                );
                context.has_custom_tools = true;
                continue;
            }
            context.custom_tools.insert(
                name.to_string(),
                CodexCustomToolSpec {
                    openai_name: name.to_string(),
                    kind: CodexCustomToolKind::Raw,
                    proxy_action: None,
                },
            );
            context.has_custom_tools = true;
            continue;
        }
        let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or("");
        match tool_type {
            "custom" => {
                let Some(name) = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                else {
                    continue;
                };
                let kind = detect_codex_custom_tool_kind(tool, name);
                context.custom_tools.insert(
                    name.to_string(),
                    CodexCustomToolSpec {
                        openai_name: name.to_string(),
                        kind,
                        proxy_action: None,
                    },
                );
                if kind == CodexCustomToolKind::ApplyPatch {
                    for action in [
                        CodexPatchProxyAction::AddFile,
                        CodexPatchProxyAction::DeleteFile,
                        CodexPatchProxyAction::UpdateFile,
                        CodexPatchProxyAction::ReplaceFile,
                        CodexPatchProxyAction::Batch,
                    ] {
                        let proxy_name = format!("{name}_{}", action.suffix());
                        context.custom_tools.insert(
                            proxy_name,
                            CodexCustomToolSpec {
                                openai_name: name.to_string(),
                                kind: CodexCustomToolKind::ApplyPatch,
                                proxy_action: Some(action),
                            },
                        );
                    }
                }
                context.has_custom_tools = true;
            }
            "function" => {
                if let Some(name) = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                {
                    context.function_tools.insert(
                        name.to_string(),
                        CodexFunctionToolSpec {
                            name: name.to_string(),
                            namespace: String::new(),
                        },
                    );
                }
            }
            "namespace" => add_namespace_tools_to_context(&mut context, tool),
            // Codex 的 tool_search（MCP 延迟加载检索）是客户端执行的代理工具：
            // 转发层把它当作 custom 代理工具登记，模型调用回来时还原成
            // tool_search_call item，检索本身仍由 Codex 客户端执行。
            "tool_search" => {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                    .unwrap_or("tool_search");
                context.custom_tools.insert(
                    name.to_string(),
                    CodexCustomToolSpec {
                        openai_name: name.to_string(),
                        kind: CodexCustomToolKind::ToolSearch,
                        proxy_action: None,
                    },
                );
                context.has_custom_tools = true;
            }
            "web_search" | "local_shell" | "computer_use" => {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                    .unwrap_or(tool_type);
                context.custom_tools.insert(
                    name.to_string(),
                    CodexCustomToolSpec {
                        openai_name: name.to_string(),
                        kind: CodexCustomToolKind::BuiltIn,
                        proxy_action: None,
                    },
                );
                context.has_custom_tools = true;
            }
            _ => {}
        }
    }

    context
}

fn add_namespace_tools_to_context(context: &mut CodexToolContext, namespace_tool: &Value) {
    let namespace = namespace_tool
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let Some(children) = namespace_tool.get("tools").and_then(Value::as_array) else {
        return;
    };
    for child in children {
        if child.get("type").and_then(Value::as_str) != Some("function") {
            continue;
        }
        let Some(name) = child
            .get("name")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        let flat = flatten_namespace_tool_name(namespace, name);
        if namespace.is_empty() {
            context.function_tools.insert(
                flat,
                CodexFunctionToolSpec {
                    namespace: namespace.to_string(),
                    name: name.to_string(),
                },
            );
        } else if context
            .function_tools
            .get(&flat)
            .is_none_or(|spec| !spec.namespace.is_empty())
        {
            context.function_tools.insert(
                flat,
                CodexFunctionToolSpec {
                    namespace: namespace.to_string(),
                    name: name.to_string(),
                },
            );
            context.has_namespace_tools = true;
        }
    }
}

fn responses_tools_to_chat_tools(tools: &[Value], context: &CodexToolContext) -> Vec<Value> {
    let mut converted = Vec::new();
    for tool in tools {
        if let Some(name) = tool.as_str().filter(|name| !name.is_empty()) {
            converted.push(generic_custom_proxy_tool(name, ""));
            continue;
        }
        match tool.get("type").and_then(Value::as_str).unwrap_or("") {
            "function" => {
                if let Some(tool) = responses_function_tool_to_chat_tool(tool) {
                    converted.push(tool);
                }
            }
            "custom" | "web_search" | "local_shell" | "computer_use" => {
                let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or("");
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                    .unwrap_or(tool_type);
                let description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if detect_codex_custom_tool_kind(tool, name) == CodexCustomToolKind::ApplyPatch {
                    converted.extend(apply_patch_proxy_tools(name, description));
                } else {
                    converted.push(generic_custom_proxy_tool(name, description));
                }
            }
            "namespace" => converted.extend(namespace_tool_to_chat_tools(tool, context)),
            // tool_search 透传为 chat 的 function 工具，名字保持 tool_search，
            // 检索由 Codex 客户端执行（execution: client），转发层只做搬运。
            "tool_search" => {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                    .unwrap_or("tool_search");
                let description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let parameters = tool.get("parameters").cloned().unwrap_or_else(|| json!({}));
                converted.push(json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": description,
                        "parameters": parameters
                    }
                }));
            }
            _ => {}
        }
    }
    converted
}

fn detect_codex_custom_tool_kind(tool: &Value, name: &str) -> CodexCustomToolKind {
    if name == "apply_patch" {
        return CodexCustomToolKind::ApplyPatch;
    }
    if let Some(definition) = tool.pointer("/format/definition").and_then(Value::as_str) {
        if definition.contains("begin_patch")
            && definition.contains("end_patch")
            && definition.contains("add_hunk")
        {
            return CodexCustomToolKind::ApplyPatch;
        }
    }
    if matches!(
        tool.get("type").and_then(Value::as_str),
        Some("web_search" | "local_shell" | "computer_use")
    ) {
        CodexCustomToolKind::BuiltIn
    } else {
        CodexCustomToolKind::Raw
    }
}

fn responses_function_tool_to_chat_tool(tool: &Value) -> Option<Value> {
    if tool.get("type").and_then(Value::as_str) != Some("function") {
        return None;
    }
    if tool.get("function").is_some() {
        let mut chat_tool = tool.clone();
        if let Some(strict) = tool.get("strict").cloned() {
            if let Some(function) = chat_tool.get_mut("function").and_then(Value::as_object_mut) {
                function.entry("strict".to_string()).or_insert(strict);
            }
            if let Some(object) = chat_tool.as_object_mut() {
                object.remove("strict");
            }
        }
        if let Some(function) = chat_tool.get_mut("function").and_then(Value::as_object_mut) {
            let normalized =
                normalize_chat_tool_parameters(function.get("parameters").unwrap_or(&json!({})));
            function.insert("parameters".to_string(), normalized);
        }
        return Some(chat_tool);
    }
    let mut function = json!({
        "name": tool.get("name").and_then(Value::as_str).unwrap_or(""),
        "description": tool.get("description").cloned().unwrap_or(Value::Null),
        "parameters": normalize_chat_tool_parameters(tool.get("parameters").unwrap_or(&json!({})))
    });
    if let Some(strict) = tool.get("strict") {
        function["strict"] = strict.clone();
    }
    Some(json!({
        "type": "function",
        "function": function
    }))
}

fn namespace_tool_to_chat_tools(namespace_tool: &Value, context: &CodexToolContext) -> Vec<Value> {
    let namespace = namespace_tool
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let namespace_description = namespace_tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let Some(children) = namespace_tool.get("tools").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut converted = Vec::new();
    for child in children {
        if child.get("type").and_then(Value::as_str) != Some("function") {
            continue;
        }
        let Some(name) = child
            .get("name")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        let flat = flatten_namespace_tool_name(namespace, name);
        if namespace != ""
            && context
                .function_tools
                .get(&flat)
                .is_some_and(|spec| spec.namespace.is_empty())
        {
            continue;
        }
        let description = combine_namespace_description(
            namespace_description,
            child
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        let mut function = json!({
            "name": flat,
            "parameters": normalize_chat_tool_parameters(child.get("parameters").unwrap_or(&json!({})))
        });
        if !description.is_empty() {
            function["description"] = json!(description);
        }
        converted.push(json!({
            "type": "function",
            "function": function
        }));
    }
    converted
}

fn normalize_chat_tool_parameters(parameters: &Value) -> Value {
    let mut normalized = if parameters.is_object() {
        parameters.clone()
    } else {
        json!({})
    };
    // 裸 `$ref` 已经是完整 schema，补默认字段会人为制造 sibling。
    let is_bare_ref = normalized
        .as_object()
        .is_some_and(|object| object.len() == 1 && object.contains_key("$ref"));
    if !is_bare_ref {
        // `type: null` 与缺失等价：严格供应商（如 deepseek）会以
        // `got 'type: null'` 拒绝整个请求（issue #2247）。
        if normalized.get("type").is_none_or(Value::is_null) {
            normalized["type"] = json!("object");
        }
        if normalized.get("properties").is_none() {
            normalized["properties"] = json!({});
        }
        if normalized.get("required").is_none() {
            normalized["required"] = json!([]);
        }
    }
    let normalized = inline_ref_siblings(&normalized);
    // 必须先内联再摊平：合并分支时 $defs 已摊开才拿得到真实属性（issue #2367）。
    flatten_top_level_combinators(normalized)
}

/// JSON Schema 的顶层组合器（`oneOf` / `anyOf` / `allOf`）。
const SCHEMA_COMBINATOR_KEYS: [&str; 3] = ["oneOf", "anyOf", "allOf"];

/// 摊平工具 schema **顶层**的组合器（issue #2367）。
///
/// zod-to-json-schema 会生成形如
/// `{type:"object", properties:{}, oneOf:[{$ref:"#/$defs/__schema0"},…], $defs:{…}}`
/// 的 schema。部分上游（Anthropic 系）拒绝顶层 `oneOf`，直接整轮 400、模型完全不可用。
/// 全仓原本对 `oneOf` 零处理。
///
/// 策略（保守优先，绝不让整轮失败）：
/// 1. 顶层有组合器时，逐分支归一化后再摊平；
/// 2. `properties` 取各分支并集，同名属性都是 object 时递归合并其 properties；
///    `required` 取交集（只有所有分支都要求才算必须）；
/// 3. 剥掉组合器键，补回 `type:"object"`；
/// 4. 无法摊平（分支不是对象、合并后 properties 为空）时把各分支塞进一个带
///    description 的私有字段，保住信息且 schema 仍合法；
/// 5. 结果仍不是合法对象 schema 时原样返回——宁可交给上游判断，也不静默丢掉工具。
fn flatten_top_level_combinators(schema: Value) -> Value {
    let Some(object) = schema.as_object() else {
        return schema;
    };
    let Some((key, branches)) = SCHEMA_COMBINATOR_KEYS.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(Value::as_array)
            .map(|a| (*key, a))
    }) else {
        return schema;
    };
    if branches.is_empty() {
        return schema;
    }

    // 分支常是裸 `$ref`（`{ "$ref": "#/$defs/__schema0" }`），它自己不带 `$defs`，
    // 所以必须拿**父级**的 $defs 来解析——只对分支单独调 inline_ref_siblings
    // 永远解析不出来（实测：分支落进降级路径，properties 为空）。
    let defs = object.get("$defs").and_then(Value::as_object);

    let mut properties = Map::new();
    let mut required: Option<BTreeSet<String>> = None;
    let mut flattenable = true;

    for branch in branches {
        let normalized = match resolve_local_definition(
            branch
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(local_definition_name)
                .unwrap_or(""),
            defs,
            &mut Vec::new(),
        ) {
            // 分支是裸 $ref：用父级 $defs 解析出真实 schema。
            Ok(Some(resolved)) if branch.as_object().is_some_and(|o| o.len() == 1) => {
                normalize_schema_value(&resolved, defs, &mut Vec::new()).unwrap_or(resolved)
            }
            _ => inline_ref_siblings(branch),
        };
        let Some(branch_object) = normalized.as_object() else {
            flattenable = false;
            break;
        };
        let branch_properties = branch_object.get("properties").and_then(Value::as_object);
        let branch_required: BTreeSet<String> = branch_object
            .get("required")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        // 分支没有 properties 时（例如 type:"string"）无法并入对象 schema。
        if branch_properties.is_none() && !branch_required.is_empty() {
            flattenable = false;
            break;
        }
        for (name, value) in branch_properties.into_iter().flatten() {
            merge_schema_property(&mut properties, name, value);
        }
        required = Some(match required {
            None => branch_required,
            Some(current) => current.intersection(&branch_required).cloned().collect(),
        });
    }

    if !flattenable || properties.is_empty() {
        // 降级：把分支原样塞进一个带说明的字段，schema 依然合法。
        let mut fallback = object.clone();
        fallback.insert("type".to_string(), json!("object"));
        fallback.insert("properties".to_string(), json!({}));
        let mut description: Vec<String> = Vec::new();
        for branch in branches {
            if let Some(text) = branch.get("description").and_then(Value::as_str) {
                if !text.trim().is_empty() {
                    description.push(text.trim().to_string());
                }
            }
        }
        fallback.insert(
            "x-merged-combinator".to_string(),
            json!({
                "kind": key,
                "branches": branches,
                "description": description.join("\n")
            }),
        );
        for combinator in SCHEMA_COMBINATOR_KEYS {
            fallback.remove(combinator);
        }
        return Value::Object(fallback);
    }

    let mut flattened = object.clone();
    for combinator in SCHEMA_COMBINATOR_KEYS {
        flattened.remove(combinator);
    }
    flattened.insert("type".to_string(), json!("object"));
    flattened.insert("properties".to_string(), Value::Object(properties));
    flattened.insert(
        "required".to_string(),
        json!(required.unwrap_or_default().into_iter().collect::<Vec<_>>()),
    );
    Value::Object(flattened)
}

/// 把分支属性并入目标 properties；同名且两侧都是 object schema 时递归合并
/// （properties 并集、required 取交集），否则保留先到的一方（非破坏性）。
fn merge_schema_property(properties: &mut Map<String, Value>, name: &str, value: &Value) {
    let Some(existing) = properties.get_mut(name) else {
        properties.insert(name.to_string(), value.clone());
        return;
    };
    let (Some(left), Some(right)) = (existing.as_object(), value.as_object()) else {
        return;
    };
    if left.get("properties").is_none() || right.get("properties").is_none() {
        return;
    }
    let mut merged = left.clone();
    let mut merged_properties = left
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (child_name, child_value) in right
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        merge_schema_property(&mut merged_properties, child_name, child_value);
    }
    merged.insert("properties".to_string(), Value::Object(merged_properties));

    let left_required: BTreeSet<String> = left
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let right_required: BTreeSet<String> = right
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    merged.insert(
        "required".to_string(),
        json!(
            left_required
                .intersection(&right_required)
                .cloned()
                .collect::<Vec<_>>()
        ),
    );
    *existing = Value::Object(merged);
}

fn inline_ref_siblings(root: &Value) -> Value {
    let defs = root.get("$defs").and_then(Value::as_object);
    let mut resolving = Vec::new();
    normalize_schema_value(root, defs, &mut resolving).unwrap_or_else(|_| root.clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalRefNormalizationError {
    Cycle,
}

fn normalize_schema_value(
    node: &Value,
    defs: Option<&Map<String, Value>>,
    resolving: &mut Vec<String>,
) -> Result<Value, LocalRefNormalizationError> {
    match node {
        Value::Array(items) => Ok(Value::Array(
            items
                .iter()
                .map(|item| normalize_schema_value(item, defs, resolving))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::Object(object) => normalize_schema_object(object, defs, resolving),
        _ => Ok(node.clone()),
    }
}

fn normalize_schema_object(
    object: &Map<String, Value>,
    defs: Option<&Map<String, Value>>,
    resolving: &mut Vec<String>,
) -> Result<Value, LocalRefNormalizationError> {
    if object.len() > 1
        && let Some(reference) = object.get("$ref").and_then(Value::as_str)
        && let Some(name) = local_definition_name(reference)
    {
        match resolve_local_definition(name, defs, resolving)? {
            Some(Value::Object(mut merged)) if merged.get("$ref").is_none() => {
                for (key, value) in object {
                    if key != "$ref" {
                        merged.insert(key.clone(), normalize_schema_value(value, defs, resolving)?);
                    }
                }
                return Ok(Value::Object(merged));
            }
            Some(_) | None => {}
        }
    }

    let mut normalized = Map::new();
    for (key, value) in object {
        // JSON Schema 的 type 必须是字符串，null 恒非法，剥掉等价于未声明。
        if key == "type" && value.is_null() {
            continue;
        }
        normalized.insert(key.clone(), normalize_schema_value(value, defs, resolving)?);
    }
    Ok(Value::Object(normalized))
}

fn resolve_local_definition(
    name: &str,
    defs: Option<&Map<String, Value>>,
    resolving: &mut Vec<String>,
) -> Result<Option<Value>, LocalRefNormalizationError> {
    let Some(defs) = defs else {
        return Ok(None);
    };
    let Some(target) = defs.get(name) else {
        return Ok(None);
    };
    if resolving.iter().any(|current| current == name) {
        return Err(LocalRefNormalizationError::Cycle);
    }

    resolving.push(name.to_string());
    let resolved = if let Some(alias) = bare_local_ref_name(target) {
        resolve_local_definition(alias, Some(defs), resolving)
    } else {
        normalize_schema_value(target, Some(defs), resolving).map(Some)
    };
    resolving.pop();
    resolved
}

fn bare_local_ref_name(node: &Value) -> Option<&str> {
    let object = node.as_object()?;
    if object.len() != 1 {
        return None;
    }
    local_definition_name(object.get("$ref")?.as_str()?)
}

fn local_definition_name(reference: &str) -> Option<&str> {
    let name = reference.strip_prefix("#/$defs/")?;
    if name.is_empty() || name.contains('/') {
        return None;
    }
    Some(name)
}

fn generic_custom_proxy_tool(name: &str, description: &str) -> Value {
    let description = if description.trim().is_empty() {
        format!("FREEFORM custom tool: {name}. Put only the tool input text here.")
    } else {
        format!(
            "{}\n\nThis is a FREEFORM tool. Do not wrap the input in JSON or markdown.",
            description.trim()
        )
    };
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "Raw freeform input for this custom tool."
                    }
                },
                "required": ["input"]
            }
        }
    })
}

fn apply_patch_proxy_tools(name: &str, description: &str) -> Vec<Value> {
    vec![
        function_tool(
            &format!("{name}_add_file"),
            &patch_proxy_description(
                description,
                "add_file",
                "Create one new file by providing a target path and full file content.",
            ),
            apply_patch_add_file_schema(),
        ),
        function_tool(
            &format!("{name}_delete_file"),
            &patch_proxy_description(
                description,
                "delete_file",
                "Delete one file by providing a target path.",
            ),
            apply_patch_delete_file_schema(),
        ),
        function_tool(
            &format!("{name}_update_file"),
            &patch_proxy_description(
                description,
                "update_file",
                "Edit one existing file with structured hunks.",
            ),
            apply_patch_update_file_schema(),
        ),
        function_tool(
            &format!("{name}_replace_file"),
            &patch_proxy_description(
                description,
                "replace_file",
                "Replace one existing file by providing a target path and full new file content.",
            ),
            apply_patch_replace_file_schema(),
        ),
        function_tool(
            &format!("{name}_batch"),
            &patch_proxy_description(
                description,
                "batch",
                "Edit files by providing structured JSON patch operations.",
            ),
            apply_patch_batch_schema(),
        ),
    ]
}

fn function_tool(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters
        }
    })
}

/// 从历史里的 tool_search_output item 收集 Codex 客户端检索命中的命名空间工具。
///
/// 条目形如 `{ type: "namespace", name, description, tools: [...] }`，必须走
/// `add_namespace_tools_to_context` + `namespace_tool_to_chat_tools` 展开成
/// `mcp__<server>__<tool>`。只取 namespace 名字的话上游只能看到外壳，
/// 调不到内层工具，反向转换也还原不出 namespace 字段。
fn collect_tool_search_output_namespaces(body: &Value) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let Some(items) = body.get("input").and_then(Value::as_array) else {
        return collected;
    };

    for item in items {
        if item.get("type").and_then(Value::as_str) != Some("tool_search_output") {
            continue;
        }
        let Some(tools) = item.get("tools").and_then(Value::as_array) else {
            continue;
        };
        for tool in tools {
            let Some(name) = tool
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            if seen.insert(name.to_string()) {
                collected.push(tool.clone());
            }
        }
    }

    collected
}

/// chat tools 按函数名去重，保留首次出现。
fn dedup_chat_tools_by_name(tools: &mut Vec<Value>) {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    tools.retain(|tool| {
        match tool
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
        {
            Some(name) => seen.insert(name.to_string()),
            None => true,
        }
    });
}

fn patch_proxy_description(description: &str, action: &str, default_description: &str) -> String {
    if description.trim().is_empty() {
        default_description.to_string()
    } else {
        format!("{} (proxy action: {action})", description.trim())
    }
}

fn apply_patch_add_file_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "path": { "type": "string", "description": "Target file path." },
            "content": { "type": "string", "description": "Full file content without patch '+' prefixes." }
        },
        "required": ["path", "content"]
    })
}

fn apply_patch_delete_file_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "path": { "type": "string", "description": "Target file path." }
        },
        "required": ["path"]
    })
}

fn apply_patch_update_file_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "path": { "type": "string", "description": "Target file path." },
            "move_to": { "type": "string", "description": "Optional destination path for move operations." },
            "hunks": apply_patch_hunks_schema()
        },
        "required": ["path", "hunks"]
    })
}

fn apply_patch_replace_file_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "path": { "type": "string", "description": "Target file path." },
            "content": { "type": "string", "description": "Full replacement content." }
        },
        "required": ["path", "content"]
    })
}

fn apply_patch_batch_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "operations": {
                "type": "array",
                "description": "Ordered list of file patch operations.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "type": { "type": "string", "enum": ["add_file", "delete_file", "update_file", "replace_file"] },
                        "path": { "type": "string" },
                        "move_to": { "type": "string", "description": "Optional destination path for move operations (update_file only)." },
                        "content": { "type": "string", "description": "Full file content for add_file / replace_file." },
                        "hunks": apply_patch_hunks_schema()
                    },
                    "required": ["type", "path"]
                }
            }
        },
        "required": ["operations"]
    })
}

fn apply_patch_hunks_schema() -> Value {
    json!({
        "type": "array",
        "description": "Structured update hunks (required when type=update_file).",
        "items": {
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "context": { "type": "string", "description": "Optional @@ context header text." },
                "lines": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "op": { "type": "string", "enum": ["context", "add", "remove"] },
                            "text": { "type": "string" }
                        },
                        "required": ["op", "text"]
                    }
                }
            },
            "required": ["lines"]
        }
    })
}

fn proxy_action_from_upstream_name(name: &str) -> Option<CodexPatchProxyAction> {
    if name.ends_with("_add_file") {
        Some(CodexPatchProxyAction::AddFile)
    } else if name.ends_with("_delete_file") {
        Some(CodexPatchProxyAction::DeleteFile)
    } else if name.ends_with("_update_file") {
        Some(CodexPatchProxyAction::UpdateFile)
    } else if name.ends_with("_replace_file") {
        Some(CodexPatchProxyAction::ReplaceFile)
    } else if name.ends_with("_batch") {
        Some(CodexPatchProxyAction::Batch)
    } else {
        None
    }
}

fn combine_namespace_description(namespace_description: &str, child_description: &str) -> String {
    let namespace_description = namespace_description.trim();
    let child_description = child_description.trim();
    match (
        namespace_description.is_empty(),
        child_description.is_empty(),
    ) {
        (true, true) => String::new(),
        (true, false) => child_description.to_string(),
        (false, true) => namespace_description.to_string(),
        (false, false) => format!("{namespace_description}\n\n{child_description}"),
    }
}

fn flatten_namespace_tool_name(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        return name.to_string();
    }
    if name.is_empty() {
        return namespace.to_string();
    }
    if namespace.ends_with("__") || name.starts_with("__") {
        format!("{namespace}{name}")
    } else {
        format!("{namespace}__{name}")
    }
}

fn responses_tool_choice_to_chat(tool_choice: &Value, context: &CodexToolContext) -> Option<Value> {
    match tool_choice {
        Value::Object(object) if object.get("type").and_then(Value::as_str) == Some("function") => {
            if let Some(namespace) = object.get("namespace").and_then(Value::as_str) {
                let name = object.get("name").and_then(Value::as_str).unwrap_or("");
                return Some(json!({
                    "type": "function",
                    "function": {
                        "name": flatten_namespace_tool_name(namespace, name)
                    }
                }));
            }
            if let Some(function) = object.get("function").and_then(Value::as_object) {
                if let Some(namespace) = function.get("namespace").and_then(Value::as_str) {
                    let name = function.get("name").and_then(Value::as_str).unwrap_or("");
                    return Some(json!({
                        "type": "function",
                        "function": {
                            "name": flatten_namespace_tool_name(namespace, name)
                        }
                    }));
                }
            }
            Some(json!({
                "type": "function",
                "function": {
                    "name": object.get("name").and_then(Value::as_str).unwrap_or("")
                }
            }))
        }
        Value::Object(object) if object.get("type").and_then(Value::as_str) == Some("custom") => {
            let name = object.get("name").and_then(Value::as_str)?;
            let spec = context.custom_tools.get(name)?;
            let upstream_name = if spec.kind == CodexCustomToolKind::ApplyPatch {
                format!("{}_batch", spec.openai_name)
            } else {
                spec.openai_name.clone()
            };
            Some(json!({
                "type": "function",
                "function": { "name": upstream_name }
            }))
        }
        other => Some(other.clone()),
    }
}

fn chat_reasoning_to_response_output_item(message: &Value, response_id: &str) -> Option<Value> {
    let reasoning = chat_reasoning_text(message)?;
    if reasoning.is_empty() {
        return None;
    }
    Some(json!({
        "id": format!("rs_{response_id}"),
        "type": "reasoning",
        "reasoning_content": reasoning,
        "summary": [{ "type": "summary_text", "text": reasoning }]
    }))
}

fn chat_reasoning_text(message: &Value) -> Option<String> {
    if let Some(reasoning) = extract_reasoning_field_text(message) {
        return Some(reasoning);
    }

    if let Some(content) = message.get("content").and_then(Value::as_str) {
        if let Some((reasoning, _answer)) = split_leading_think_block(content) {
            if !reasoning.is_empty() {
                return Some(reasoning);
            }
        }
    }

    None
}

fn chat_message_to_response_output_item(message: &Value, response_id: &str) -> Option<Value> {
    let mut content = Vec::new();
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        let text = split_leading_think_block(text)
            .map(|(_reasoning, answer)| answer)
            .unwrap_or_else(|| text.to_string());
        if !text.is_empty() {
            content.push(json!({ "type": "output_text", "text": text, "annotations": [] }));
        }
    } else if let Some(parts) = message.get("content").and_then(Value::as_array) {
        for part in parts {
            match part.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" | "output_text" => {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            content.push(
                                json!({ "type": "output_text", "text": text, "annotations": [] }),
                            );
                        }
                    }
                }
                "refusal" => {
                    if let Some(refusal) = part.get("refusal").and_then(Value::as_str) {
                        if !refusal.is_empty() {
                            content.push(json!({ "type": "refusal", "refusal": refusal }));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(refusal) = message.get("refusal").and_then(Value::as_str) {
        if !refusal.is_empty() {
            content.push(json!({ "type": "refusal", "refusal": refusal }));
        }
    }

    if content.is_empty() {
        return None;
    }

    Some(json!({
        "id": format!("msg_{}", response_id_body(response_id)),
        "type": "message",
        "status": "completed",
        "role": "assistant",
        "content": content
    }))
}

fn chat_tool_calls_to_response_output_items(
    message: &Value,
    tool_context: &CodexToolContext,
) -> Vec<Value> {
    let mut output = Vec::new();
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for (index, tool_call) in tool_calls.iter().enumerate() {
            output.push(chat_tool_call_to_response_item(
                tool_call,
                index,
                tool_context,
            ));
        }
    } else if let Some(function_call) = message.get("function_call") {
        output.push(chat_legacy_function_call_to_response_item(
            function_call,
            tool_context,
        ));
    }
    output
}

fn chat_tool_call_to_response_item(
    tool_call: &Value,
    index: usize,
    tool_context: &CodexToolContext,
) -> Value {
    let call_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("call_{index}"));
    let function = tool_call.get("function").unwrap_or(&Value::Null);
    let name = function.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = responses_arguments_to_chat(function.get("arguments").unwrap_or(&json!({})));
    response_tool_call_item(&call_id, name, &arguments, tool_context)
}

fn chat_legacy_function_call_to_response_item(
    function_call: &Value,
    tool_context: &CodexToolContext,
) -> Value {
    let call_id = function_call
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("call_0");
    let name = function_call
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let arguments =
        responses_arguments_to_chat(function_call.get("arguments").unwrap_or(&json!({})));
    response_tool_call_item(call_id, name, &arguments, tool_context)
}

fn tool_call_added_item(
    state: &ToolCallState,
    output_index: u32,
    tool_context: &CodexToolContext,
) -> Value {
    if tool_context.is_custom_tool_proxy(&state.name) {
        if tool_context.is_tool_search_proxy(&state.name) {
            return json!({
                "type": "response.output_item.added",
                "output_index": output_index,
                "item": {
                    "id": tool_call_item_id(&state.call_id, &state.name, tool_context),
                    "type": "tool_search_call",
                    "status": "in_progress",
                    "call_id": state.call_id,
                    "execution": "client",
                    "arguments": {}
                }
            });
        }
        return json!({
            "type": "response.output_item.added",
            "output_index": output_index,
            "item": {
                "id": tool_call_item_id(&state.call_id, &state.name, tool_context),
                "type": "custom_tool_call",
                "status": "in_progress",
                "call_id": state.call_id,
                "name": tool_context.original_custom_tool_name(&state.name),
                "input": ""
            }
        });
    }
    let (display_name, namespace) = tool_context.openai_name_for_function_tool(&state.name);
    let mut item = json!({
        "type": "response.output_item.added",
        "output_index": output_index,
        "item": {
            "id": state.item_id,
            "type": "function_call",
            "status": "in_progress",
            "call_id": state.call_id,
            "name": display_name,
            "arguments": ""
        }
    });
    if !namespace.is_empty() {
        item["item"]["namespace"] = json!(namespace);
    }
    item
}

fn push_tool_call_delta_sse(
    output: &mut String,
    state: &ToolCallState,
    output_index: u32,
    delta: &str,
    tool_context: &CodexToolContext,
) {
    if tool_context.is_tool_search_proxy(&state.name) {
        // tool_search 走 function 参数流，客户端按 tool_search_call.arguments 聚合。
        push_sse(
            output,
            "response.function_call_arguments.delta",
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": state.item_id,
                "output_index": output_index,
                "delta": delta
            }),
        );
    } else if tool_context.is_custom_tool_proxy(&state.name) {
        let _ = delta;
    } else {
        push_sse(
            output,
            "response.function_call_arguments.delta",
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": state.item_id,
                "output_index": output_index,
                "delta": delta
            }),
        );
    }
}

fn push_tool_call_done_sse(
    output: &mut String,
    state: &ToolCallState,
    output_index: u32,
    tool_context: &CodexToolContext,
) {
    if tool_context.is_tool_search_proxy(&state.name) {
        push_sse(
            output,
            "response.function_call_arguments.done",
            json!({
                "type": "response.function_call_arguments.done",
                "item_id": state.item_id,
                "output_index": output_index,
                "arguments": state.arguments
            }),
        );
        return;
    }
    if tool_context.is_custom_tool_proxy(&state.name) {
        push_sse(
            output,
            "response.custom_tool_call_input.delta",
            json!({
                "type": "response.custom_tool_call_input.delta",
                "item_id": tool_call_item_id(&state.call_id, &state.name, tool_context),
                "call_id": state.call_id,
                "output_index": output_index,
                "delta": reconstruct_custom_tool_call_input_with_context(
                    tool_context,
                    &state.name,
                    &state.arguments
                )
            }),
        );
        return;
    }
    push_sse(
        output,
        "response.function_call_arguments.done",
        json!({
            "type": "response.function_call_arguments.done",
            "item_id": state.item_id,
            "output_index": output_index,
            "arguments": state.arguments
        }),
    );
}

fn tool_call_done_item(state: &ToolCallState, tool_context: &CodexToolContext) -> Value {
    response_tool_call_item(&state.call_id, &state.name, &state.arguments, tool_context)
}

fn response_tool_call_item(
    call_id: &str,
    name: &str,
    arguments: &str,
    tool_context: &CodexToolContext,
) -> Value {
    if tool_context.is_custom_tool_proxy(name) {
        if tool_context.is_tool_search_proxy(name) {
            // 官方客户端的 tool_search handler 只接受 tool_search_call item，
            // function_call 形态会被拒绝，因此必须还原成专属 item 类型；
            // arguments 转发层原样搬运（对象字符串双向保真）。
            return json!({
                "id": format!("tsc_{call_id}"),
                "type": "tool_search_call",
                "status": "completed",
                "call_id": call_id,
                "execution": "client",
                "arguments": responses_arguments_to_chat_parse(arguments)
            });
        }
        return json!({
            "id": tool_call_item_id(call_id, name, tool_context),
            "type": "custom_tool_call",
            "status": "completed",
            "call_id": call_id,
            "name": tool_context.original_custom_tool_name(name),
            "input": reconstruct_custom_tool_call_input_with_context(tool_context, name, arguments)
        });
    }
    let (display_name, namespace) = tool_context.openai_name_for_function_tool(name);
    let mut item = json!({
        "id": format!("fc_{call_id}"),
        "type": "function_call",
        "status": "completed",
        "call_id": call_id,
        "name": display_name,
        "arguments": arguments
    });
    if !namespace.is_empty() {
        item["namespace"] = json!(namespace);
    }
    item
}

fn tool_call_item_id(call_id: &str, name: &str, tool_context: &CodexToolContext) -> String {
    let prefix = if tool_context.is_custom_tool_proxy(name) {
        if tool_context.is_tool_search_proxy(name) {
            // 官方客户端给 tool_search_call 分配的 item id 前缀（见 codex id_prefix）。
            return format!("tsc_{call_id}");
        }
        "ctc_"
    } else {
        "fc_"
    };
    format!("{prefix}{call_id}")
}

fn split_leading_think_block(text: &str) -> Option<(String, String)> {
    let leading_ws_len = text.len() - text.trim_start().len();
    let after_ws = &text[leading_ws_len..];
    if !after_ws.starts_with(THINK_OPEN_TAG) {
        return None;
    }
    let body_start = leading_ws_len + THINK_OPEN_TAG.len();
    let close_relative = text[body_start..].find(THINK_CLOSE_TAG)?;
    let close_start = body_start + close_relative;
    let answer_start = close_start + THINK_CLOSE_TAG.len();
    Some((
        text[body_start..close_start].trim().to_string(),
        strip_think_answer_separator(&text[answer_start..]).to_string(),
    ))
}

fn strip_leading_think_open_tag(text: &str) -> Option<String> {
    let leading_ws_len = text.len() - text.trim_start().len();
    let after_ws = &text[leading_ws_len..];
    after_ws
        .strip_prefix(THINK_OPEN_TAG)
        .map(|value| value.trim().to_string())
}

fn strip_think_answer_separator(text: &str) -> &str {
    text.trim_start_matches(['\r', '\n', '\t', ' '])
}

fn extract_reasoning_field_text(value: &Value) -> Option<String> {
    for key in ["reasoning_content", "reasoning"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }

    if let Some(reasoning) = value.get("reasoning") {
        for key in ["content", "text", "summary"] {
            if let Some(text) = reasoning.get(key).and_then(Value::as_str) {
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }

    value
        .get("reasoning_details")
        .and_then(extract_reasoning_details_text)
}

fn extract_reasoning_details_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => (!text.is_empty()).then(|| text.to_string()),
        Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(extract_reasoning_detail_part_text)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            (!text.is_empty()).then_some(text)
        }
        Value::Object(_) => extract_reasoning_detail_part_text(value),
        _ => None,
    }
}

fn extract_reasoning_detail_part_text(value: &Value) -> Option<String> {
    for key in ["text", "content", "summary"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }

    if let Some(parts) = value.get("parts").and_then(Value::as_array) {
        let text = parts
            .iter()
            .filter_map(extract_reasoning_detail_part_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        return (!text.is_empty()).then_some(text);
    }

    None
}

fn extract_reasoning_summary_text(value: &Value) -> Option<String> {
    for key in ["reasoning_content", "content", "text"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }

    let summary = value.get("summary")?;
    if let Some(text) = summary.as_str() {
        return (!text.is_empty()).then(|| text.to_string());
    }

    let parts = summary.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .or_else(|| part.get("content").and_then(Value::as_str))
                .or_else(|| part.as_str())
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    (!text.is_empty()).then_some(text)
}

fn default_responses_usage() -> Value {
    // Codex 把 output_tokens_details.reasoning_tokens 当必填解析,
    // 兜底 usage 也必须带齐该结构。
    json!({
        "input_tokens": 0,
        "output_tokens": 0,
        "total_tokens": 0,
        "output_tokens_details": { "reasoning_tokens": 0 }
    })
}

fn chat_usage_to_responses_usage(usage: Option<&Value>) -> Value {
    let Some(usage) = usage.filter(|value| value.is_object() && !value.is_null()) else {
        return default_responses_usage();
    };
    let mut input_tokens = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .or_else(|| usage.get("promptTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut input_tokens_include_cache = usage.get("prompt_tokens").is_some();
    let output_tokens = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .or_else(|| usage.get("candidatesTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut cached_tokens = usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .or_else(|| usage.pointer("/input_tokens_details/cached_tokens"))
        .or_else(|| usage.get("cachedContentTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_creation_5m = usage
        .get("cache_creation_5m_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_creation_1h = usage
        .get("cache_creation_1h_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let has_claude_cache_fields = usage.get("cache_read_input_tokens").is_some()
        || usage.get("cache_creation_input_tokens").is_some()
        || usage.get("cache_creation_5m_input_tokens").is_some()
        || usage.get("cache_creation_1h_input_tokens").is_some();
    let has_cache_details = cached_tokens > 0
        || usage
            .pointer("/prompt_tokens_details/cached_tokens")
            .is_some()
        || usage
            .pointer("/input_tokens_details/cached_tokens")
            .is_some();

    if let Some(value) = usage.get("input_tokens").and_then(Value::as_u64) {
        input_tokens = value;
        input_tokens_include_cache = false;
    }
    if let Some(cache_read) = usage.get("cache_read_input_tokens").and_then(Value::as_u64) {
        cached_tokens = cache_read;
    }
    if let Some(prompt_tokens) = usage.get("promptTokenCount").and_then(Value::as_u64) {
        cached_tokens = usage
            .get("cachedContentTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        input_tokens = prompt_tokens.saturating_sub(cached_tokens);
        input_tokens_include_cache = false;
    }

    let usage_input_tokens = if input_tokens_include_cache {
        input_tokens.saturating_sub(
            cached_tokens
                + effective_cache_creation_tokens(
                    cache_creation,
                    cache_creation_5m,
                    cache_creation_1h,
                ),
        )
    } else {
        input_tokens
    };
    let should_recalculate_total = usage.get("total_tokens").is_none()
        || cached_tokens > 0
        || effective_cache_creation_tokens(cache_creation, cache_creation_5m, cache_creation_1h)
            > 0
        || usage.get("promptTokenCount").is_some();
    let total_tokens = if should_recalculate_total {
        usage_input_tokens
            + output_tokens
            + cached_tokens
            + effective_cache_creation_tokens(cache_creation, cache_creation_5m, cache_creation_1h)
    } else {
        usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(usage_input_tokens + output_tokens)
    };
    let mut result = json!({
        "input_tokens": usage_input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": total_tokens
    });

    if !has_claude_cache_fields && has_cache_details && cached_tokens > 0 {
        result["input_tokens_details"] = json!({ "cached_tokens": cached_tokens });
    }
    if let Some(details) = usage.get("completion_tokens_details") {
        // Codex parses output_tokens_details.reasoning_tokens as a required field;
        // upstreams (e.g. Kimi) omit the key when a response had no reasoning,
        // which makes the Responses client fail with "missing field
        // `reasoning_tokens`" and abort the whole turn. Default it to 0.
        let mut details = details.clone();
        if details.is_object() && details.get("reasoning_tokens").is_none() {
            details["reasoning_tokens"] = json!(0);
        }
        result["output_tokens_details"] = details;
    } else {
        // 上游连 completion_tokens_details 都没给时同样补全, 避免
        // Codex 解析 response.completed 时缺字段断流。
        result["output_tokens_details"] = json!({ "reasoning_tokens": 0 });
    }
    if let Some(cache_read) = usage.get("cache_read_input_tokens") {
        result["cache_read_input_tokens"] = cache_read.clone();
    }
    if let Some(cache_creation) = usage.get("cache_creation_input_tokens") {
        result["cache_creation_input_tokens"] = cache_creation.clone();
    }
    if let Some(cache_creation) = usage.get("cache_creation_5m_input_tokens") {
        result["cache_creation_5m_input_tokens"] = cache_creation.clone();
    }
    if let Some(cache_creation) = usage.get("cache_creation_1h_input_tokens") {
        result["cache_creation_1h_input_tokens"] = cache_creation.clone();
    }
    let cache_ttl = match (cache_creation_5m > 0, cache_creation_1h > 0) {
        (true, true) => Some("mixed"),
        (true, false) => Some("5m"),
        (false, true) => Some("1h"),
        (false, false) => None,
    };
    if let Some(cache_ttl) = cache_ttl {
        result["cache_ttl"] = json!(cache_ttl);
    }
    result
}

fn effective_cache_creation_tokens(
    cache_creation: u64,
    cache_creation_5m: u64,
    cache_creation_1h: u64,
) -> u64 {
    if cache_creation > 0 {
        cache_creation
    } else {
        cache_creation_5m + cache_creation_1h
    }
}

fn response_status(finish_reason: Option<&str>) -> &'static str {
    match finish_reason {
        Some("length") => "incomplete",
        _ => "completed",
    }
}

fn response_output_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => canonical_json_string(other),
    }
}

const IMAGE_DATA_URL_PREFIX: &str = "data:image/";

/// 若 `text` 含 base64 图片 data URL，返回替换成占位符后的文本；否则 `None`。
fn redact_image_data_urls(text: &str) -> Option<String> {
    if !text.contains(IMAGE_DATA_URL_PREFIX) {
        return None;
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(IMAGE_DATA_URL_PREFIX) {
        out.push_str(&rest[..start]);
        out.push_str("[image omitted]");
        let tail = &rest[start..];
        // data URL 由 base64 字母表加少量分隔符组成，遇到其它字符即结束。
        let end = tail
            .find(|c: char| {
                !(c.is_ascii_alphanumeric()
                    || matches!(c, '+' | '/' | '=' | ':' | ';' | ',' | '.' | '-' | '_'))
            })
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    Some(out)
}

/// base64 图片一旦落进文本字段就是灾难：上游会把它当普通文本 tokenize，而 base64
/// 高熵、几乎没有可复用的 BPE merge（约 1.36 字符/token），一张 2MB 的图能膨胀到
/// 约 200 万 token 并直接撑爆上下文窗口。图片的唯一合法归宿是 `image_url` 子树。
///
/// 这是最后一道兜底：出站前扫一遍 body，把漏进文本字段的 data URL 换成占位符。
/// 命中即说明某条协议转换路径有 bug，记诊断日志便于定位。
fn guard_inline_image_data_urls(value: &mut Value) -> bool {
    match value {
        Value::Object(map) => {
            let mut hit = false;
            for (key, child) in map.iter_mut() {
                // image_url 子树是图片的合法归宿，跳过。
                if key == "image_url" {
                    continue;
                }
                hit |= guard_inline_image_data_urls(child);
            }
            hit
        }
        Value::Array(items) => {
            let mut hit = false;
            for item in items {
                hit |= guard_inline_image_data_urls(item);
            }
            hit
        }
        Value::String(text) => match redact_image_data_urls(text) {
            Some(cleaned) => {
                *text = cleaned;
                true
            }
            None => false,
        },
        _ => false,
    }
}

/// tool 输出可能带图 —— `view_image` 的结果就是
/// `function_call_output.output[] = [{"type":"input_image","image_url":"data:image/png;base64,…"}]`。
///
/// 直接走 `response_output_text` 会把整个数组 JSON 序列化成字符串，于是 base64 被当作
/// 普通文本送进上游 tokenizer。base64 是 BPE 最不擅长的输入（高熵、无可复用 merge，
/// 约 1.36 字符/token），一张 2MB 的 PNG 因此膨胀到约 200 万 token 并撑爆上下文窗口；
/// 同一张图走 `image_url` 只需几百 token，因为供应商在 tokenize 之前就把 base64 解码回
/// 像素、按尺寸切 patch 计数。
///
/// 所以这里在**有图时**保留结构化的 `image_url` part，交给
/// `relocate_tool_output_images` 在满足 tool 配对约束的前提下搬到后续 user 消息。
/// 无图时原样返回 `response_output_text` 的结果，保持既有行为不变。
fn tool_output_content(output: &Value) -> Value {
    let Some(parts) = output.as_array() else {
        return json!(response_output_text(output));
    };
    if !parts.iter().any(is_image_part) {
        return json!(response_output_text(output));
    }

    let mut chat_parts = Vec::new();
    for part in parts {
        if is_image_part(part) {
            if let Some(image) = image_part_to_chat(part) {
                chat_parts.push(image);
            }
            continue;
        }
        let text = match part.get("type").and_then(Value::as_str) {
            Some("input_text") | Some("output_text") | Some("text") => part
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            // 非文本非图片的块（结构化工具结果等）保留原有的 JSON 表示，
            // 免得静默丢信息。
            _ => canonical_json_string(part),
        };
        if !text.is_empty() {
            chat_parts.push(json!({ "type": "text", "text": text }));
        }
    }
    Value::Array(chat_parts)
}

fn build_custom_tool_call_history(name: &str, input: &Value) -> (String, String) {
    let input = response_output_text(input);
    if name == "apply_patch" || input.starts_with("*** Begin Patch") {
        let operations = parse_apply_patch_operations(&input);
        if operations.len() == 1 {
            let action = operations[0]
                .get("type")
                .and_then(Value::as_str)
                .and_then(single_apply_patch_action)
                .unwrap_or(CodexPatchProxyAction::Batch);
            return (
                format!("{name}_{}", action.suffix()),
                build_apply_patch_operation_arguments(&operations[0], action),
            );
        }
        return (
            format!("{name}_batch"),
            json!({ "operations": operations, "raw_patch": input }).to_string(),
        );
    }
    (name.to_string(), json!({ "input": input }).to_string())
}

fn reconstruct_custom_tool_call_input_with_context(
    tool_context: &CodexToolContext,
    upstream_name: &str,
    arguments: &str,
) -> String {
    if let Some(spec) = tool_context.custom_tools.get(upstream_name) {
        if spec.kind == CodexCustomToolKind::ApplyPatch {
            return reconstruct_apply_patch_input(spec.proxy_action, arguments);
        }
    }
    reconstruct_custom_tool_call_input(arguments)
}

fn reconstruct_custom_tool_call_input(arguments: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(arguments) else {
        return arguments.to_string();
    };
    value
        .get("input")
        .map(response_output_text)
        .unwrap_or_else(|| arguments.to_string())
}

fn reconstruct_apply_patch_input(action: Option<CodexPatchProxyAction>, arguments: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(arguments) else {
        return arguments.to_string();
    };
    if let Some(raw_patch) = value
        .get("raw_patch")
        .or_else(|| value.get("patch"))
        .or_else(|| value.get("input"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return raw_patch.to_string();
    }

    let operations = match action.unwrap_or(CodexPatchProxyAction::Batch) {
        CodexPatchProxyAction::AddFile => vec![json!({
            "type": "add_file",
            "path": value.get("path").and_then(Value::as_str).unwrap_or(""),
            "content": value.get("content").and_then(Value::as_str).unwrap_or("")
        })],
        CodexPatchProxyAction::DeleteFile => vec![json!({
            "type": "delete_file",
            "path": value.get("path").and_then(Value::as_str).unwrap_or("")
        })],
        CodexPatchProxyAction::UpdateFile => vec![json!({
            "type": "update_file",
            "path": value.get("path").and_then(Value::as_str).unwrap_or(""),
            "move_to": value.get("move_to").and_then(Value::as_str).unwrap_or(""),
            "hunks": value.get("hunks").cloned().unwrap_or_else(|| json!([]))
        })],
        CodexPatchProxyAction::ReplaceFile => vec![json!({
            "type": "replace_file",
            "path": value.get("path").and_then(Value::as_str).unwrap_or(""),
            "content": value.get("content").and_then(Value::as_str).unwrap_or("")
        })],
        CodexPatchProxyAction::Batch => value
            .get("operations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };

    build_apply_patch_text(&operations)
}

fn build_apply_patch_text(operations: &[Value]) -> String {
    let mut text = String::from("*** Begin Patch");
    for operation in operations {
        let op_type = operation.get("type").and_then(Value::as_str).unwrap_or("");
        let path = operation.get("path").and_then(Value::as_str).unwrap_or("");
        match op_type {
            "add_file" => {
                text.push_str(&format!("\n*** Add File: {path}"));
                for line in operation
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .lines()
                {
                    text.push_str("\n+");
                    text.push_str(line);
                }
            }
            "delete_file" => {
                text.push_str(&format!("\n*** Delete File: {path}"));
            }
            "update_file" => {
                text.push_str(&format!("\n*** Update File: {path}"));
                if let Some(move_to) = operation.get("move_to").and_then(Value::as_str) {
                    if !move_to.is_empty() {
                        text.push_str(&format!("\n*** Move to: {move_to}"));
                    }
                }
                if let Some(hunks) = operation.get("hunks").and_then(Value::as_array) {
                    for hunk in hunks {
                        let context = hunk.get("context").and_then(Value::as_str).unwrap_or("");
                        if context.is_empty() {
                            text.push_str("\n@@");
                        } else {
                            text.push_str(&format!("\n@@ {context}"));
                        }
                        if let Some(lines) = hunk.get("lines").and_then(Value::as_array) {
                            for line in lines {
                                text.push('\n');
                                text.push_str(line_op_prefix(
                                    line.get("op").and_then(Value::as_str).unwrap_or("context"),
                                ));
                                text.push_str(
                                    line.get("text").and_then(Value::as_str).unwrap_or(""),
                                );
                            }
                        }
                    }
                }
            }
            "replace_file" => {
                text.push_str(&format!("\n*** Delete File: {path}"));
                text.push_str(&format!("\n*** Add File: {path}"));
                for line in operation
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .lines()
                {
                    text.push_str("\n+");
                    text.push_str(line);
                }
            }
            _ => {}
        }
    }
    text.push_str("\n*** End Patch");
    text
}

fn line_op_prefix(op: &str) -> &'static str {
    match op {
        "add" => "+",
        "remove" | "delete" => "-",
        _ => " ",
    }
}

fn parse_apply_patch_operations(input: &str) -> Vec<Value> {
    let mut operations = Vec::new();
    let mut current: Option<serde_json::Map<String, Value>> = None;
    let mut content_lines: Vec<String> = Vec::new();
    let mut hunks: Vec<Value> = Vec::new();
    let mut current_hunk: Option<serde_json::Map<String, Value>> = None;
    let mut hunk_lines: Vec<Value> = Vec::new();

    let flush_hunk = |current_hunk: &mut Option<serde_json::Map<String, Value>>,
                      hunk_lines: &mut Vec<Value>,
                      hunks: &mut Vec<Value>| {
        if let Some(mut hunk) = current_hunk.take() {
            hunk.insert("lines".to_string(), json!(std::mem::take(hunk_lines)));
            hunks.push(Value::Object(hunk));
        }
    };
    let flush_operation = |current: &mut Option<serde_json::Map<String, Value>>,
                           content_lines: &mut Vec<String>,
                           hunks: &mut Vec<Value>,
                           operations: &mut Vec<Value>| {
        if let Some(mut operation) = current.take() {
            match operation.get("type").and_then(Value::as_str).unwrap_or("") {
                "add_file" | "replace_file" => {
                    operation.insert("content".to_string(), json!(content_lines.join("\n")));
                }
                "update_file" => {
                    operation.insert("hunks".to_string(), json!(std::mem::take(hunks)));
                }
                _ => {}
            }
            content_lines.clear();
            operations.push(Value::Object(operation));
        }
    };

    for raw_line in input.lines() {
        if raw_line == "*** Begin Patch" || raw_line == "*** End Patch" {
            continue;
        }
        if let Some(path) = raw_line.strip_prefix("*** Add File: ") {
            flush_hunk(&mut current_hunk, &mut hunk_lines, &mut hunks);
            flush_operation(
                &mut current,
                &mut content_lines,
                &mut hunks,
                &mut operations,
            );
            current = Some(serde_json::Map::from_iter([
                ("type".to_string(), json!("add_file")),
                ("path".to_string(), json!(path)),
            ]));
            continue;
        }
        if let Some(path) = raw_line.strip_prefix("*** Delete File: ") {
            flush_hunk(&mut current_hunk, &mut hunk_lines, &mut hunks);
            flush_operation(
                &mut current,
                &mut content_lines,
                &mut hunks,
                &mut operations,
            );
            current = Some(serde_json::Map::from_iter([
                ("type".to_string(), json!("delete_file")),
                ("path".to_string(), json!(path)),
            ]));
            continue;
        }
        if let Some(path) = raw_line.strip_prefix("*** Update File: ") {
            flush_hunk(&mut current_hunk, &mut hunk_lines, &mut hunks);
            flush_operation(
                &mut current,
                &mut content_lines,
                &mut hunks,
                &mut operations,
            );
            current = Some(serde_json::Map::from_iter([
                ("type".to_string(), json!("update_file")),
                ("path".to_string(), json!(path)),
            ]));
            continue;
        }
        if let Some(path) = raw_line.strip_prefix("*** Move to: ") {
            if let Some(operation) = current.as_mut() {
                operation.insert("move_to".to_string(), json!(path));
            }
            continue;
        }
        if raw_line.starts_with("@@") {
            flush_hunk(&mut current_hunk, &mut hunk_lines, &mut hunks);
            let context = raw_line.strip_prefix("@@").unwrap_or("").trim().to_string();
            current_hunk = Some(serde_json::Map::from_iter([(
                "context".to_string(),
                json!(context),
            )]));
            continue;
        }
        if let Some(operation) = current.as_ref() {
            match operation.get("type").and_then(Value::as_str).unwrap_or("") {
                "add_file" | "replace_file" => {
                    if let Some(line) = raw_line.strip_prefix('+') {
                        content_lines.push(line.to_string());
                    }
                }
                "update_file" => {
                    let (op, text) = match raw_line.chars().next() {
                        Some('+') => ("add", &raw_line[1..]),
                        Some('-') => ("remove", &raw_line[1..]),
                        Some(' ') => ("context", &raw_line[1..]),
                        _ => ("context", raw_line),
                    };
                    hunk_lines.push(json!({ "op": op, "text": text }));
                }
                _ => {}
            }
        }
    }

    flush_hunk(&mut current_hunk, &mut hunk_lines, &mut hunks);
    flush_operation(
        &mut current,
        &mut content_lines,
        &mut hunks,
        &mut operations,
    );
    operations
}

fn single_apply_patch_action(op_type: &str) -> Option<CodexPatchProxyAction> {
    match op_type {
        "add_file" => Some(CodexPatchProxyAction::AddFile),
        "delete_file" => Some(CodexPatchProxyAction::DeleteFile),
        "update_file" => Some(CodexPatchProxyAction::UpdateFile),
        "replace_file" => Some(CodexPatchProxyAction::ReplaceFile),
        _ => None,
    }
}

fn build_apply_patch_operation_arguments(
    operation: &Value,
    action: CodexPatchProxyAction,
) -> String {
    match action {
        CodexPatchProxyAction::AddFile | CodexPatchProxyAction::ReplaceFile => json!({
            "content": operation.get("content").and_then(Value::as_str).unwrap_or(""),
            "path": operation.get("path").and_then(Value::as_str).unwrap_or("")
        })
        .to_string(),
        CodexPatchProxyAction::DeleteFile => json!({
            "path": operation.get("path").and_then(Value::as_str).unwrap_or("")
        })
        .to_string(),
        CodexPatchProxyAction::UpdateFile => {
            let mut args = json!({
                "hunks": operation.get("hunks").cloned().unwrap_or_else(|| json!([])),
                "path": operation.get("path").and_then(Value::as_str).unwrap_or("")
            });
            if let Some(move_to) = operation.get("move_to").and_then(Value::as_str) {
                if !move_to.is_empty() {
                    args["move_to"] = json!(move_to);
                }
            }
            args.to_string()
        }
        CodexPatchProxyAction::Batch => json!({ "operations": [operation.clone()] }).to_string(),
    }
}

fn copy_response_request_fields(response: &mut Value, original_request: Option<&Value>) {
    let Some(original_request) = original_request else {
        return;
    };
    for key in [
        "instructions",
        "max_output_tokens",
        "parallel_tool_calls",
        "previous_response_id",
        "reasoning",
        "temperature",
        "tool_choice",
        "tools",
        "top_p",
        "metadata",
    ] {
        if let Some(value) = original_request.get(key) {
            response[key] = value.clone();
        }
    }
}

fn responses_arguments_to_chat(value: &Value) -> String {
    match value {
        Value::String(text) => normalize_chat_tool_arguments_string(text),
        Value::Object(_) => canonical_json_string(value),
        Value::Null => "{}".to_string(),
        other => canonical_json_string(&json!({ "input": other })),
    }
}

/// 把 chat 侧的 arguments 字符串解析回 JSON Value，供需要对象形态参数的
/// item（如 tool_search_call）使用；解析失败时退回空对象，避免构造非法 item。
fn responses_arguments_to_chat_parse(arguments: &str) -> Value {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return json!({});
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| json!({}))
}

fn normalize_chat_tool_arguments_string(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "{}".to_string();
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(Value::Object(_)) => trimmed.to_string(),
        Ok(value) => canonical_json_string(&json!({ "input": value })),
        Err(_) => canonical_json_string(&json!({ "input": text })),
    }
}

fn instruction_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| part.as_str())
            })
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        other => other.as_str().unwrap_or_default().to_string(),
    }
}

fn canonical_json_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap_or_default(),
        Value::Array(values) => {
            let parts = values.iter().map(canonical_json_string).collect::<Vec<_>>();
            format!("[{}]", parts.join(","))
        }
        Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| *key);
            let parts = entries
                .into_iter()
                .map(|(key, value)| {
                    let key = serde_json::to_string(key).unwrap_or_default();
                    format!("{key}:{}", canonical_json_string(value))
                })
                .collect::<Vec<_>>();
            format!("{{{}}}", parts.join(","))
        }
    }
}

fn apply_chat_reasoning_options(result: &mut Value, body: &Value, model: &str, standard: bool) {
    let Some(reasoning_enabled) = reasoning_requested(body) else {
        return;
    };
    let style = if standard {
        ChatReasoningStyle::Default
    } else {
        infer_chat_reasoning_style(model)
    };

    match style {
        ChatReasoningStyle::Thinking => {
            result["thinking"] = json!({
                "type": if reasoning_enabled {
                    kimi_thinking_enabled_type(model)
                } else {
                    "disabled"
                }
            });
        }
        ChatReasoningStyle::EnableThinking => {
            result["enable_thinking"] = json!(reasoning_enabled);
        }
        ChatReasoningStyle::ReasoningSplit => {
            result["reasoning_split"] = json!(reasoning_enabled);
        }
        _ => {}
    }

    if !reasoning_enabled {
        if style == ChatReasoningStyle::OpenRouter {
            result["reasoning"] = json!({ "effort": "none" });
        }
        return;
    }

    let Some(effort) = body.pointer("/reasoning/effort").and_then(Value::as_str) else {
        return;
    };
    let Some(mapped) = map_chat_reasoning_effort(effort, style) else {
        return;
    };

    match style {
        ChatReasoningStyle::OpenRouter => {
            result["reasoning"] = json!({ "effort": mapped });
        }
        ChatReasoningStyle::DeepSeek
        | ChatReasoningStyle::LowHigh
        | ChatReasoningStyle::Default
            if supports_reasoning_effort(model) =>
        {
            result["reasoning_effort"] = json!(mapped);
        }
        // Kimi For Coding (K3 / K2.7 Code): 官方接受 reasoning_effort 三档
        // low/high/max (默认 high), 且服务端会把 medium→high、xhigh→max。
        // 仅限 for-coding 模型 ID, 避免给 glm/mimo/kimi-k2 等其它
        // Thinking 方言上游误发该字段。
        ChatReasoningStyle::Thinking if is_kimi_coding_model(model) => {
            result["reasoning_effort"] = json!(mapped);
        }
        _ => {}
    }
}

fn reasoning_requested(body: &Value) -> Option<bool> {
    if let Some(effort) = body.pointer("/reasoning/effort").and_then(Value::as_str) {
        return Some(!matches!(
            effort.trim().to_ascii_lowercase().as_str(),
            "none" | "off" | "disabled"
        ));
    }

    body.get("reasoning").map(|value| !value.is_null())
}

fn infer_chat_reasoning_style(model: &str) -> ChatReasoningStyle {
    let model = model.to_ascii_lowercase();
    if model.contains("openrouter") || model.starts_with("openrouter/") {
        return ChatReasoningStyle::OpenRouter;
    }
    if model.contains("deepseek") {
        return ChatReasoningStyle::DeepSeek;
    }
    if model.contains("qwen") || model.contains("dashscope") || model.contains("bailian") {
        return ChatReasoningStyle::EnableThinking;
    }
    if model.contains("kimi")
        || model.contains("moonshot")
        || model.starts_with("k3")
        || model.contains("glm")
        || model.contains("zhipu")
        || model.contains("z.ai")
        || model.contains("mimo")
    {
        return ChatReasoningStyle::Thinking;
    }
    if model.contains("minimax") {
        return ChatReasoningStyle::ReasoningSplit;
    }
    if model.contains("siliconflow") {
        return ChatReasoningStyle::EnableThinking;
    }
    if model.contains("stepfun") || model.contains("step-3.5-flash-2603") {
        return ChatReasoningStyle::LowHigh;
    }
    ChatReasoningStyle::Default
}

fn map_chat_reasoning_effort(effort: &str, style: ChatReasoningStyle) -> Option<&'static str> {
    let effort = effort.trim().to_ascii_lowercase();
    if matches!(effort.as_str(), "none" | "off" | "disabled") {
        return None;
    }

    match style {
        ChatReasoningStyle::DeepSeek => match effort.as_str() {
            "max" | "xhigh" => Some("max"),
            _ => Some("high"),
        },
        ChatReasoningStyle::LowHigh => match effort.as_str() {
            "minimal" | "low" => Some("low"),
            _ => Some("high"),
        },
        ChatReasoningStyle::OpenRouter => match effort.as_str() {
            "max" | "xhigh" => Some("xhigh"),
            "high" => Some("high"),
            "medium" => Some("medium"),
            "low" => Some("low"),
            "minimal" => Some("minimal"),
            _ => None,
        },
        // Kimi For Coding 官方映射: minimal/low→low, medium/high→high,
        // xhigh/max→max。注意不能直接透传 "minimal", 服务端不认会 400。
        ChatReasoningStyle::Thinking => match effort.as_str() {
            "minimal" | "low" => Some("low"),
            "medium" | "high" => Some("high"),
            "xhigh" | "max" => Some("max"),
            _ => None,
        },
        _ => match effort.as_str() {
            "minimal" => Some("minimal"),
            "low" => Some("low"),
            "medium" => Some("medium"),
            "high" => Some("high"),
            "xhigh" => Some("xhigh"),
            "max" => Some("max"),
            _ => None,
        },
    }
}

/// Kimi For Coding 专属模型 ID(k3 / k3-256k / kimi-for-coding[-highspeed])。
/// 只有这些上游接受 `reasoning_effort` 三档; kimi-k2-thinking 等旧模型
/// 仍只发 thinking 开关。
fn is_kimi_coding_model(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.starts_with("k3") || model.contains("kimi-k3") || model.contains("for-coding")
}

fn kimi_thinking_enabled_type(model: &str) -> &'static str {
    if is_kimi_coding_model(model) {
        "adaptive"
    } else {
        "enabled"
    }
}

fn supports_reasoning_effort(model: &str) -> bool {
    is_openai_o_series(model)
        || model
            .to_lowercase()
            .strip_prefix("gpt-")
            .and_then(|rest| rest.chars().next())
            .is_some_and(|ch| ch.is_ascii_digit() && ch >= '5')
        || infer_chat_reasoning_style(model) == ChatReasoningStyle::DeepSeek
        || infer_chat_reasoning_style(model) == ChatReasoningStyle::LowHigh
}

fn is_openai_o_series(model: &str) -> bool {
    model.len() > 1
        && model.starts_with('o')
        && model
            .as_bytes()
            .get(1)
            .is_some_and(|byte| byte.is_ascii_digit())
}

/// 供应商自定义请求头必须真正写进发往上游的请求（issue #1685）。
#[cfg(test)]
mod relay_custom_header_tests {
    use super::*;
    use crate::settings::{RelayHeaderKeyValue, RelayMode, RelayProfile};

    fn header(key: &str, value: &str) -> RelayHeaderKeyValue {
        RelayHeaderKeyValue {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    fn relay_with(headers: Vec<RelayHeaderKeyValue>) -> RelayProfile {
        RelayProfile {
            relay_mode: RelayMode::PureApi,
            api_key: "sk-upstream".to_string(),
            custom_headers: headers,
            ..RelayProfile::default()
        }
    }

    fn build_upstream_request(relay: &RelayProfile) -> reqwest::Request {
        upstream_request_builder(
            reqwest::Client::new(),
            "http://upstream.example/v1/responses",
            relay,
            false,
            &serde_json::json!({ "model": "m" }),
        )
        .build()
        .unwrap()
    }

    #[test]
    fn upstream_request_carries_custom_headers() {
        let request = build_upstream_request(&relay_with(vec![header("X-Tenant", "acme")]));
        assert_eq!(request.headers().get("x-tenant").unwrap(), "acme");
        assert_eq!(
            request.headers().get("authorization").unwrap(),
            "Bearer sk-upstream"
        );
    }

    /// 代理路径与测试连接、模型列表一致：显式 Authorization 优先于 API Key。
    #[test]
    fn upstream_request_prefers_custom_authorization() {
        let request = build_upstream_request(&relay_with(vec![header(
            "Authorization",
            "Bearer explicit",
        )]));
        assert_eq!(request.headers().get_all("authorization").iter().count(), 1);
        assert_eq!(
            request.headers().get("authorization").unwrap(),
            "Bearer explicit"
        );
    }
}

#[cfg(test)]
mod channel_cooldown_retry_tests {
    use super::*;

    #[test]
    fn cooldown_allows_three_automatic_retries_only() {
        assert!(should_retry_after_cooldown(0));
        assert!(should_retry_after_cooldown(1));
        assert!(should_retry_after_cooldown(2));
        assert!(!should_retry_after_cooldown(3));
        assert!(!should_retry_after_cooldown(usize::MAX));
    }
}
