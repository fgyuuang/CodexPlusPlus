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
use serde_json::{Value, json};

use crate::relay_rotation::{RotationContext, RotationEvent};
use crate::settings::{RelayProtocol, SettingsStore};

pub const DEFAULT_PROTOCOL_PROXY_PORT: u16 = 57321;
pub const OFFICIAL_CHATGPT_CODEX_RESPONSES_URL: &str =
    "https://chatgpt.com/backend-api/codex/responses";
pub const OFFICIAL_CHATGPT_CODEX_IMAGE_GENERATIONS_URL: &str =
    "https://chatgpt.com/backend-api/codex/images/generations";
pub const OFFICIAL_CHATGPT_CODEX_IMAGE_EDITS_URL: &str =
    "https://chatgpt.com/backend-api/codex/images/edits";
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const UPSTREAM_HEADER_TIMEOUT: Duration = Duration::from_secs(30);
const UPSTREAM_STREAM_HEADER_TIMEOUT: Duration = Duration::from_secs(120);
const UPSTREAM_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const UPSTREAM_BODY_TIMEOUT: Duration = Duration::from_secs(120);
const UPSTREAM_IMAGE_HEADER_TIMEOUT: Duration = Duration::from_secs(300);
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
    enforce_tool_call_pairing(&mut messages);
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

    apply_chat_reasoning_options(&mut result, &body, model);

    let tool_context = build_codex_tool_context(body.get("tools"));
    let mut has_chat_tools = false;
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        let converted = responses_tools_to_chat_tools(tools, &tool_context);
        if !converted.is_empty() {
            has_chat_tools = true;
            result["tools"] = json!(converted);
        }
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
    let context = build_codex_tool_context(original_request.get("tools"));
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
    /// 底层请求命中容量错误，调用方应在代理内重新发起请求。
    pub capacity_retryable: bool,
    /// 当前 Responses 请求启用了容量错误改写。
    pub capacity_retry_enabled: bool,
    /// 用于在读取到非流式上游错误体后继续执行容量重试计数的请求指纹。
    pub capacity_retry_key: Option<u64>,
    pub capacity_retry_max_attempts: u8,
    /// 已读取但尚未转发的流首段，用于容量错误探测后继续转发正常流。
    pub prefetched_chunk: Vec<u8>,
    pub response: reqwest::Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum UpstreamWireApi {
    Responses,
    ChatCompletions,
    AudioTranscriptions,
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
    let settings = SettingsStore::default().load().unwrap_or_default();
    open_responses_proxy_request_with_settings_and_capacity_retries_and_user_agent(
        body,
        settings,
        original_user_agent,
        request_path,
        OFFICIAL_CHATGPT_CODEX_RESPONSES_URL,
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
    )
    .await
}

async fn open_responses_proxy_request_with_settings_and_capacity_retries_and_user_agent(
    body: &str,
    settings: crate::settings::BackendSettings,
    original_user_agent: Option<&str>,
    request_path: &str,
    official_endpoint: &str,
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
            self.state.finalize_into(&mut output);
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

    fn observe_block(&mut self, block: &str) {
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
        if event_name.is_some_and(is_terminal_responses_event) {
            self.terminal = true;
            return;
        }
        let data = data_parts.join("\n");
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
    let settings = SettingsStore::default().load().unwrap_or_default();
    open_official_images_proxy_request_with_settings_and_endpoint_and_user_agent(
        body,
        settings,
        operation,
        operation.official_endpoint(),
        original_user_agent,
    )
    .await
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
    )
    .await
}

async fn open_official_images_proxy_request_with_settings_and_endpoint_and_user_agent(
    body: &str,
    settings: crate::settings::BackendSettings,
    operation: ImageProxyOperation,
    official_endpoint: &str,
    original_user_agent: Option<&str>,
) -> anyhow::Result<UpstreamProxyResponse> {
    if !settings.official_login_mixed_mode {
        anyhow::bail!("官方图像代理仅在官方登录混合模式下启用");
    }
    let request_json: Value = serde_json::from_str(body).context("官方图像请求必须是 JSON")?;
    let auth = resolve_official_chatgpt_auth(&settings)?;
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
        response: upstream,
    })
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
    let settings = SettingsStore::default().load().unwrap_or_default();
    open_responses_proxy_request_with_settings_and_user_agent(
        body,
        settings,
        original_user_agent,
        request_path,
    )
    .await
}

pub async fn open_responses_proxy_request_with_settings(
    body: &str,
    settings: crate::settings::BackendSettings,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_with_settings_and_user_agent(body, settings, None, "/responses")
        .await
}

pub async fn open_responses_proxy_request_with_settings_for_path(
    body: &str,
    settings: crate::settings::BackendSettings,
    request_path: &str,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_with_settings_and_user_agent(body, settings, None, request_path)
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
    )
    .await
}

async fn open_responses_proxy_request_with_settings_and_user_agent(
    body: &str,
    settings: crate::settings::BackendSettings,
    original_user_agent: Option<&str>,
    request_path: &str,
) -> anyhow::Result<UpstreamProxyResponse> {
    open_responses_proxy_request_with_settings_and_user_agent_and_official_endpoint(
        body,
        settings,
        original_user_agent,
        request_path,
        OFFICIAL_CHATGPT_CODEX_RESPONSES_URL,
    )
    .await
}

async fn open_responses_proxy_request_with_settings_and_user_agent_and_official_endpoint(
    body: &str,
    settings: crate::settings::BackendSettings,
    original_user_agent: Option<&str>,
    request_path: &str,
    official_endpoint: &str,
) -> anyhow::Result<UpstreamProxyResponse> {
    let mut request_json: Value = serde_json::from_str(body)?;
    let capacity_retry_key = settings
        .codex_app_capacity_retry
        .then(|| capacity_retry_request_key(&request_json));
    let is_stream = request_json
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let source_model = request_json
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let model_route = select_model_route(&settings, &source_model)?;
    if let Some(route) = &model_route
        && route.upstream_model != source_model
    {
        request_json["model"] = Value::String(route.upstream_model.clone());
    }
    let context = RotationContext {
        conversation_id: conversation_id_from_responses_request(&request_json),
    };
    let requested_model = request_json
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty());
    let (relay, relays, track_aggregate_rotation) = if let Some(route) = &model_route {
        (route.relay.clone(), vec![route.relay.clone()], false)
    } else {
        let route = crate::relay_rotation::classify_mixed_model_route(&settings, requested_model);
        let (relay, track_aggregate_rotation) = match route {
            crate::relay_rotation::MixedModelRoute::Official => {
                return open_official_chatgpt_responses_request(
                    &settings,
                    request_json,
                    is_stream,
                    original_user_agent,
                    official_endpoint,
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
    for (attempt, relay) in relays.into_iter().enumerate() {
        validate_upstream(&relay)?;
        let (endpoint, upstream_body, wire_api) =
            upstream_request_parts(&relay, request_json.clone(), request_path).await?;
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
                }))
            }),
        );
        let mut upstream = match send_upstream_request_for_responses(
            upstream_request_builder(
                crate::http_client::proxied_client(&effective_user_agent(
                    &relay.user_agent,
                    original_user_agent,
                ))?,
                &endpoint,
                relay.api_key.trim(),
                is_stream,
                &upstream_body,
            ),
            is_stream,
        )
        .await
        {
            Ok(upstream) => upstream,
            Err(error) => {
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
                            response: upstream,
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
                        capacity_retry_max_attempts: settings.codex_app_capacity_retry_max_attempts,
                        prefetched_chunk: Vec::new(),
                        response: upstream,
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
                response: upstream,
            });
        }
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

fn select_model_route(
    settings: &crate::settings::BackendSettings,
    model: &str,
) -> anyhow::Result<Option<ModelRouteSelection>> {
    if model.is_empty() || settings.active_aggregate_relay_profile().is_some() {
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

async fn open_official_chatgpt_responses_request(
    settings: &crate::settings::BackendSettings,
    mut request_json: Value,
    is_stream: bool,
    original_user_agent: Option<&str>,
    official_endpoint: &str,
) -> anyhow::Result<UpstreamProxyResponse> {
    normalize_responses_input_items(&mut request_json);
    let capacity_retry_key = settings
        .codex_app_capacity_retry
        .then(|| capacity_retry_request_key(&request_json));
    let auth = resolve_official_chatgpt_auth(settings)?;
    let configured_user_agent = settings
        .official_login_relay_profile()
        .map(|profile| profile.user_agent.as_str())
        .unwrap_or("");
    let client = crate::http_client::proxied_client(&effective_user_agent(
        configured_user_agent,
        original_user_agent,
    ))?;
    let mut builder = client
        .post(official_endpoint)
        .bearer_auth(&auth.access_token)
        .header("ChatGPT-Account-Id", &auth.account_id)
        .header("originator", "codex_cli_rs")
        .header(reqwest::header::CONTENT_TYPE, "application/json");
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
                        response: upstream,
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
                    response: upstream,
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
        response: upstream,
    })
}

fn normalize_responses_input_items(request: &mut Value) {
    let Some(items) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    items.retain(|item| {
        item.get("type").and_then(Value::as_str) != Some("reasoning")
            || item.get("reasoning_content").is_none()
            || item.get("encrypted_content").is_some()
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
    let upstream = send_upstream_request(
        crate::http_client::proxied_client(&effective_user_agent(
            &relay.user_agent,
            original_user_agent,
        ))?
        .get(endpoint)
        .bearer_auth(relay.api_key.trim()),
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
        wire_api: UpstreamWireApi::Responses,
        capacity_retryable: false,
        capacity_retry_enabled: false,
        capacity_retry_key: None,
        capacity_retry_max_attempts: 0,
        prefetched_chunk: Vec::new(),
        response: upstream,
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
    let upstream = send_upstream_request(
        crate::http_client::proxied_client(&effective_user_agent(
            &relay.user_agent,
            original_user_agent,
        ))?
        .post(endpoint)
        .bearer_auth(relay.api_key.trim())
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .body(body.to_vec()),
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
        wire_api: UpstreamWireApi::AudioTranscriptions,
        capacity_retryable: false,
        capacity_retry_enabled: false,
        capacity_retry_key: None,
        capacity_retry_max_attempts: 0,
        prefetched_chunk: Vec::new(),
        response: upstream,
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
    if relay.api_key.trim().is_empty() {
        anyhow::bail!("Chat Completions 上游 Key 不能为空");
    }

    let request_json: Value = serde_json::from_str(body)?;
    let is_stream = request_json
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let client = crate::http_client::proxied_client(&effective_user_agent(
        &relay.user_agent,
        original_user_agent,
    ))?;
    let mut builder = client
        .post(chat_completions_url(&relay.base_url))
        .bearer_auth(relay.api_key.trim())
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    if is_stream {
        builder = builder
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header(reqwest::header::CACHE_CONTROL, "no-cache");
    }
    let upstream =
        send_upstream_request_for_responses(builder.json(&request_json), is_stream).await?;
    let status_code = upstream.status().as_u16();
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();

    Ok(UpstreamProxyResponse {
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
        response: upstream,
    })
}

async fn upstream_request_parts(
    relay: &crate::settings::RelayProfile,
    request_json: Value,
    request_path: &str,
) -> anyhow::Result<(String, Value, UpstreamWireApi)> {
    let compact = is_responses_compact_proxy_path(request_path);
    if compact && relay.protocol == RelayProtocol::ChatCompletions {
        anyhow::bail!("Chat Completions 协议暂不支持 Responses compact 请求");
    }
    let mut body = match relay.protocol {
        RelayProtocol::Responses => request_json,
        RelayProtocol::ChatCompletions => responses_to_chat_completions(request_json)?,
    };
    rewrite_model_for_relay(&mut body, relay);
    if relay.protocol == RelayProtocol::Responses {
        normalize_responses_input_items(&mut body);
        normalize_responses_custom_tool_call_ids(&mut body);
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
                            .await;
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
        RelayProtocol::Responses => UpstreamWireApi::Responses,
        RelayProtocol::ChatCompletions => UpstreamWireApi::ChatCompletions,
    };
    Ok((
        match relay.protocol {
            RelayProtocol::Responses if compact => responses_compact_url(&relay.base_url),
            RelayProtocol::Responses => responses_url(&relay.base_url),
            RelayProtocol::ChatCompletions => chat_completions_url(&relay.base_url),
        },
        body,
        wire_api,
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
    api_key: &str,
    is_stream: bool,
    upstream_body: &Value,
) -> reqwest::RequestBuilder {
    let mut builder = client
        .post(endpoint)
        .bearer_auth(api_key)
        .header(reqwest::header::CONTENT_TYPE, "application/json");
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
    if relay.api_key.trim().is_empty() {
        anyhow::bail!("上游 Key 不能为空");
    }
    Ok(())
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
    let upstream_content_type = upstream.content_type.clone();
    let is_stream = upstream.is_stream;
    let wire_api = upstream.wire_api;
    let mut upstream_body = upstream.prefetched_chunk;
    upstream_body.extend_from_slice(
        &read_upstream_body_with_timeout(upstream.response, upstream_body_timeout()).await?,
    );

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
        Self {
            model,
            tool_context: build_codex_tool_context(original_request.get("tools")),
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
            let item_id = format!("{}_msg", self.response_id);
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

fn normalize_responses_custom_tool_call_ids(body: &mut Value) {
    let Some(input) = body.get_mut("input") else {
        return;
    };
    match input {
        Value::Array(items) => {
            for item in items {
                normalize_custom_tool_call_item_id(item);
            }
        }
        Value::Object(_) => normalize_custom_tool_call_item_id(input),
        _ => {}
    }
}

fn normalize_custom_tool_call_item_id(item: &mut Value) {
    if item.get("type").and_then(Value::as_str) != Some("custom_tool_call") {
        return;
    }
    let Some(id) = item.get("id").and_then(Value::as_str) else {
        return;
    };
    if id.starts_with("ctc_") {
        return;
    }
    let suffix = id
        .strip_prefix("fc_")
        .or_else(|| id.strip_prefix("item_"))
        .unwrap_or(id);
    item["id"] = json!(format!("ctc_{suffix}"));
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
    if normalized.get("type").is_none() {
        normalized["type"] = json!("object");
    }
    if normalized.get("properties").is_none() {
        normalized["properties"] = json!({});
    }
    if normalized.get("required").is_none() {
        normalized["required"] = json!([]);
    }
    normalized
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
        "id": format!("{response_id}_msg"),
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
    if tool_context.is_custom_tool_proxy(&state.name) {
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

fn apply_chat_reasoning_options(result: &mut Value, body: &Value, model: &str) {
    let Some(reasoning_enabled) = reasoning_requested(body) else {
        return;
    };
    let style = infer_chat_reasoning_style(model);

    match style {
        ChatReasoningStyle::Thinking => {
            result["thinking"] = json!({
                "type": if reasoning_enabled { "enabled" } else { "disabled" }
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
    model.starts_with("k3") || model.contains("for-coding")
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
