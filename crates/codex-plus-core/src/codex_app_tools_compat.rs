//! Codex Desktop 宿主工具的只读探针与版本判定。
//!
//! `codex_app` 是桌面宿主提供的 MCP，不属于上游模型供应商。这里仅保存工具
//! 装配的结构化摘要，禁止把 prompt、tool arguments、认证信息或任务正文写入日志。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub const CODEX_APP_MCP_SERVER: &str = "codex_app";
pub const CODEX_APP_PLUGIN_ID: &str = "codex-app-tools@openai-bundled";
pub const CODEX_APP_TOOLS_PROBE_EVENT: &str = "codex_app_tools_probe";
pub const CODEX_APP_TOOLS_PROBE_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppServerGeneration {
    Legacy,
    McpFirst,
    Unknown,
}

impl AppServerGeneration {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::McpFirst => "mcp_first",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAppToolsProbe {
    #[serde(default = "default_probe_schema_version")]
    pub schema_version: u8,
    #[serde(default)]
    pub app_server_version: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub model_provider: String,
    #[serde(default)]
    pub tool_mode: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub request_method: String,
    #[serde(default)]
    pub response_kind: String,
    #[serde(default)]
    pub mcp_server: String,
    #[serde(default)]
    pub mcp_seen: bool,
    #[serde(default)]
    pub mcp_tool_count: Option<usize>,
    #[serde(default)]
    pub mcp_has_read_thread: bool,
    #[serde(default)]
    pub legacy_dynamic_codex_app: bool,
}

fn default_probe_schema_version() -> u8 {
    CODEX_APP_TOOLS_PROBE_SCHEMA_VERSION
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAppToolsStatus {
    pub status: String,
    pub guidance: String,
    pub app_server_generation: String,
    pub probe: Option<CodexAppToolsProbe>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAppToolsFilterReport {
    pub mcp_seen: bool,
    pub legacy_dynamic_count: usize,
    pub filtered_count: usize,
}

impl CodexAppToolsFilterReport {
    pub const fn observed(self) -> bool {
        self.mcp_seen || self.legacy_dynamic_count > 0
    }

    pub const fn changed(self) -> bool {
        self.filtered_count > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexAppToolEntryKind {
    Mcp,
    LegacyDynamic,
    Other,
}

/// Removes the deprecated `codex_app` dynamic namespace only when the request
/// also contains the newer `mcp__codex_app` entry.
///
/// The proxy cannot reliably infer the app-server version from an individual
/// Responses request. The dual-entry check is therefore intentionally
/// conservative: an old-only tool list is left untouched, while a request
/// containing both generations is made safe for 0.154+ by removing only the
/// stale Codex app entries. Other dynamic tools and all MCP tools are kept.
pub fn filter_stale_codex_app_dynamic_tools(request: &mut Value) -> CodexAppToolsFilterReport {
    let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) else {
        return CodexAppToolsFilterReport::default();
    };

    let mut report = CodexAppToolsFilterReport {
        mcp_seen: tools
            .iter()
            .any(|tool| codex_app_tool_entry_kind(tool) == CodexAppToolEntryKind::Mcp),
        legacy_dynamic_count: tools
            .iter()
            .filter(|tool| codex_app_tool_entry_kind(tool) == CodexAppToolEntryKind::LegacyDynamic)
            .count(),
        filtered_count: 0,
    };

    if !report.mcp_seen || report.legacy_dynamic_count == 0 {
        return report;
    }

    let before = tools.len();
    tools.retain(|tool| codex_app_tool_entry_kind(tool) != CodexAppToolEntryKind::LegacyDynamic);
    report.filtered_count = before.saturating_sub(tools.len());
    report
}

fn codex_app_tool_entry_kind(tool: &Value) -> CodexAppToolEntryKind {
    if let Some(name) = tool.as_str() {
        return codex_app_tool_name_kind(name);
    }

    let Some(object) = tool.as_object() else {
        return CodexAppToolEntryKind::Other;
    };
    let tool_type = object.get("type").and_then(Value::as_str).unwrap_or("");

    if tool_type == "mcp"
        && ["server_label", "serverLabel", "name"]
            .iter()
            .filter_map(|key| object.get(*key).and_then(Value::as_str))
            .any(|name| name == CODEX_APP_MCP_SERVER)
    {
        return CodexAppToolEntryKind::Mcp;
    }

    for key in ["name", "namespace"] {
        if let Some(name) = object.get(key).and_then(Value::as_str) {
            let kind = codex_app_tool_name_kind(name);
            if kind != CodexAppToolEntryKind::Other {
                return kind;
            }
        }
    }

    CodexAppToolEntryKind::Other
}

fn codex_app_tool_name_kind(name: &str) -> CodexAppToolEntryKind {
    let name = name.trim();
    if name == "mcp__codex_app" || name.starts_with("mcp__codex_app__") {
        return CodexAppToolEntryKind::Mcp;
    }
    if name == CODEX_APP_MCP_SERVER || name.starts_with("codex_app__") {
        return CodexAppToolEntryKind::LegacyDynamic;
    }
    CodexAppToolEntryKind::Other
}

pub fn classify_app_server_version(version: &str) -> AppServerGeneration {
    let mut components = version
        .split(|character: char| !character.is_ascii_digit())
        .filter(|component| !component.is_empty())
        .filter_map(|component| component.parse::<u64>().ok());
    let Some(major) = components.next() else {
        return AppServerGeneration::Unknown;
    };
    let minor = components.next().unwrap_or(0);
    let patch = components.next().unwrap_or(0);

    if major == 0 && minor <= 153 {
        AppServerGeneration::Legacy
    } else if (major == 0 && minor >= 154) || major > 0 || patch > 0 {
        AppServerGeneration::McpFirst
    } else {
        AppServerGeneration::Unknown
    }
}

pub fn classify_probe(probe: &CodexAppToolsProbe) -> &'static str {
    match (
        probe.mcp_seen,
        probe.mcp_has_read_thread,
        probe.legacy_dynamic_codex_app,
    ) {
        (true, true, true)
            if classify_app_server_version(&probe.app_server_version)
                == AppServerGeneration::McpFirst =>
        {
            "legacy_dynamic_conflict"
        }
        (true, true, true) => "mcp_and_legacy",
        (true, true, false) => "mcp_ready",
        (true, false, _) => "mcp_read_thread_missing",
        (false, false, true) => "legacy_dynamic_only",
        _ => "no_codex_app_evidence",
    }
}

pub fn guidance_for_status(status: &str) -> &'static str {
    match status {
        "mcp_ready" => "codex_app MCP 已发现 read_thread。",
        "legacy_dynamic_conflict" => {
            "检测到 0.154+ 的 codex_app MCP 与已废弃 dynamic_tools.codex_app 同时存在。"
        }
        "mcp_and_legacy" => "同时发现 codex_app MCP 和旧动态入口，需要结合 app-server 版本判断。",
        "mcp_read_thread_missing" => "codex_app MCP 已启动，但工具清单没有 read_thread。",
        "legacy_dynamic_only" => "只发现旧 dynamic_tools.codex_app，尚未发现可用的 codex_app MCP。",
        _ => "尚未获得完整的 codex_app 工具装配证据，请启动一个新任务后再查看诊断。",
    }
}

pub fn status_from_probe(probe: Option<CodexAppToolsProbe>) -> CodexAppToolsStatus {
    let app_server_generation = probe
        .as_ref()
        .map(|probe| classify_app_server_version(&probe.app_server_version).as_str())
        .unwrap_or(AppServerGeneration::Unknown.as_str())
        .to_string();
    let status = probe
        .as_ref()
        .map(classify_probe)
        .unwrap_or("no_codex_app_evidence")
        .to_string();
    let guidance = guidance_for_status(&status).to_string();
    CodexAppToolsStatus {
        status,
        guidance,
        app_server_generation,
        probe,
    }
}

pub fn latest_probe_from_log(path: &Path) -> std::io::Result<Option<CodexAppToolsProbe>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    for line in text.lines().rev() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(detail) = probe_detail_from_log_record(&record) else {
            continue;
        };
        if let Ok(probe) = serde_json::from_value::<CodexAppToolsProbe>(detail.clone()) {
            return Ok(Some(probe));
        }
    }
    Ok(None)
}

fn probe_detail_from_log_record(record: &Value) -> Option<&Value> {
    match record.get("event").and_then(Value::as_str) {
        Some(CODEX_APP_TOOLS_PROBE_EVENT) => record.get("detail"),
        Some("renderer.codex_app_tools_probe") => {
            let wrapper = record.get("detail")?;
            (wrapper.get("event").and_then(Value::as_str) == Some(CODEX_APP_TOOLS_PROBE_EVENT))
                .then(|| wrapper.get("detail"))
                .flatten()
        }
        _ => None,
    }
}

pub fn latest_status() -> CodexAppToolsStatus {
    let path = crate::paths::default_diagnostic_log_path();
    let probe = latest_probe_from_log(&path).ok().flatten();
    status_from_probe(probe)
}

#[cfg(test)]
mod tests {
    use super::{
        AppServerGeneration, CodexAppToolsProbe, classify_app_server_version, classify_probe,
        filter_stale_codex_app_dynamic_tools, guidance_for_status, latest_probe_from_log,
        status_from_probe,
    };
    use serde_json::json;

    #[test]
    fn classifies_the_known_app_server_cutover() {
        assert_eq!(
            classify_app_server_version("0.153.0"),
            AppServerGeneration::Legacy
        );
        assert_eq!(
            classify_app_server_version("0.154.0-alpha.6.2"),
            AppServerGeneration::McpFirst
        );
        assert_eq!(
            classify_app_server_version("not-a-version"),
            AppServerGeneration::Unknown
        );
    }

    #[test]
    fn identifies_a_new_mcp_and_legacy_dynamic_conflict() {
        let probe = CodexAppToolsProbe {
            app_server_version: "0.154.0-alpha.6.2".to_string(),
            mcp_seen: true,
            mcp_tool_count: Some(8),
            mcp_has_read_thread: true,
            legacy_dynamic_codex_app: true,
            ..CodexAppToolsProbe::default()
        };
        assert_eq!(classify_probe(&probe), "legacy_dynamic_conflict");
        assert!(guidance_for_status(classify_probe(&probe)).contains("dynamic_tools"));
    }

    #[test]
    fn identifies_mcp_without_read_thread() {
        let probe = CodexAppToolsProbe {
            mcp_seen: true,
            mcp_tool_count: Some(2),
            ..CodexAppToolsProbe::default()
        };
        let status = status_from_probe(Some(probe));
        assert_eq!(status.status, "mcp_read_thread_missing");
        assert_eq!(status.app_server_generation, "unknown");
    }

    #[test]
    fn leaves_a_legacy_only_tool_list_untouched() {
        let mut request = json!({
            "model": "CLIProxyAPI:gpt-5.6-sol",
            "tools": [
                {
                    "type": "namespace",
                    "name": "codex_app",
                    "tools": [{"type": "function", "name": "read_thread"}]
                },
                {"type": "function", "name": "other_dynamic__lookup"}
            ]
        });
        let before = request.clone();

        let report = filter_stale_codex_app_dynamic_tools(&mut request);

        assert!(!report.mcp_seen);
        assert_eq!(report.legacy_dynamic_count, 1);
        assert_eq!(report.filtered_count, 0);
        assert_eq!(request, before);
    }

    #[test]
    fn filters_only_stale_codex_app_entries_when_mcp_is_present() {
        let mut request = json!({
            "tools": [
                {"type": "function", "name": "mcp__codex_app__read_thread"},
                {"type": "function", "name": "codex_app__read_thread"},
                {
                    "type": "namespace",
                    "name": "codex_app",
                    "tools": [{"type": "function", "name": "list_threads"}]
                },
                {"type": "function", "name": "other_dynamic__lookup"},
                {"type": "function", "name": "lookup"}
            ]
        });

        let report = filter_stale_codex_app_dynamic_tools(&mut request);

        assert!(report.mcp_seen);
        assert_eq!(report.legacy_dynamic_count, 2);
        assert_eq!(report.filtered_count, 2);
        assert_eq!(request["tools"].as_array().map(Vec::len), Some(3));
        assert!(request["tools"].as_array().unwrap().iter().all(|tool| {
            tool.get("name")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|name| !name.starts_with("codex_app"))
        }));
    }

    #[test]
    fn recognizes_an_explicit_mcp_server_descriptor() {
        let mut request = json!({
            "tools": [
                {"type": "mcp", "server_label": "codex_app"},
                {"type": "namespace", "name": "codex_app", "tools": []}
            ]
        });

        let report = filter_stale_codex_app_dynamic_tools(&mut request);

        assert!(report.mcp_seen);
        assert_eq!(report.filtered_count, 1);
        assert_eq!(request["tools"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn reads_renderer_wrapped_probe_records() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("codex-plus.log");
        std::fs::write(
            &path,
            serde_json::to_string(&json!({
                "event": "renderer.codex_app_tools_probe",
                "detail": {
                    "event": "codex_app_tools_probe",
                    "detail": {
                        "schemaVersion": 1,
                        "model": "CLIProxyAPI:gpt-5.6-sol",
                        "mcpSeen": true,
                        "mcpToolCount": 8,
                        "mcpHasReadThread": true
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let probe = latest_probe_from_log(&path).unwrap().unwrap();

        assert_eq!(probe.model, "CLIProxyAPI:gpt-5.6-sol");
        assert!(probe.mcp_seen);
        assert_eq!(probe.mcp_tool_count, Some(8));
        assert!(probe.mcp_has_read_thread);
    }
}
