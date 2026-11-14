//! Obsidian Claudian 的 Codex 后端使用独立 CODEX_HOME，不改主 Codex 的登录或会话。
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::files::{StashedFile, parse_jsonc};
use super::{AgentEnv, AgentSpec, AgentWire, FileSet, PROVIDER_ID, Wiring, object_entry};
use anyhow::{Context, ensure};
use serde_json::{Value, json};

pub static SPECS: &[AgentSpec] = &[AgentSpec {
    id: "claudian",
    name: "Claudian (Obsidian)",
    binary: "",
    wire: AgentWire::Responses,
    detect: |env| vaults(env).is_ok_and(|paths| !paths.is_empty()),
    config_path: |env| {
        vaults(env)
            .ok()
            .and_then(|paths| paths.into_iter().next())
            .map(|vault| settings_path(&vault))
            .unwrap_or_else(|| env.app_config().join("obsidian/obsidian.json"))
    },
    write,
    hint: "接入所有已登记且启用 Claudian 的 Obsidian 仓库，只配置 Codex 后端。重载 Claudian 后新建 Codex 对话并刷新模型；旧对话与主 Codex 登录保留。断开可恢复原连接。WSL 后端请先改用本机 Codex CLI。",
}];

fn settings_path(vault: &Path) -> PathBuf {
    vault.join(".claudian/claudian-settings.json")
}

fn vaults(env: &AgentEnv) -> anyhow::Result<Vec<PathBuf>> {
    let candidates = if let Some(path) = env.dir_var("CLAUDIAN_VAULT") {
        ensure!(path.is_absolute(), "CLAUDIAN_VAULT 必须是绝对路径");
        vec![path]
    } else {
        let path = env.app_config().join("obsidian/obsidian.json");
        let Some(text) = crate::gateway::restore::read_optional(&path)? else {
            return Ok(Vec::new());
        };
        let registry = parse_jsonc(&text).context("Obsidian 仓库登记文件无法解析")?;
        registry
            .get("vaults")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|map| map.values())
            .filter_map(|vault| vault["path"].as_str())
            .map(PathBuf::from)
            .collect()
    };
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for path in candidates {
        if !path.is_absolute() || !settings_path(&path).is_file() {
            continue;
        }
        let canonical = path.canonicalize().context("无法确认 Claudian 仓库路径")?;
        if seen.insert(canonical) {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn env_key(line: &str) -> Option<&str> {
    let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
    if line.starts_with('#') {
        return None;
    }
    line.split_once('=').map(|(key, _)| key.trim())
}

fn with_home(text: &str, home: &Path) -> anyhow::Result<String> {
    let home = home
        .to_str()
        .context("Claudian CODEX_HOME 不是有效 Unicode 路径")?;
    ensure!(
        !home.contains(['\r', '\n', '"']),
        "Claudian 仓库路径包含无法写入环境变量的字符"
    );
    let mut lines: Vec<_> = text
        .lines()
        .filter(|line| env_key(line) != Some("CODEX_HOME"))
        .map(str::to_string)
        .collect();
    lines.push(format!("CODEX_HOME=\"{home}\""));
    Ok(lines.join("\n"))
}

fn write(env: &AgentEnv, wiring: &Wiring, files: &mut FileSet) -> anyhow::Result<()> {
    let vaults = vaults(env)?;
    ensure!(!vaults.is_empty(), "未找到已配置 Claudian 的 Obsidian 仓库");
    ensure!(
        !wiring.default_model.is_empty(),
        "请先为 Claudian 选择网关默认模型"
    );
    for vault in vaults {
        let home = vault.join(".claudian/codex-plus");
        let original = std::fs::read_to_string(settings_path(&vault))?;
        let original = parse_jsonc(&original).context("Claudian 设置无法解析")?;
        ensure!(
            original.is_object()
                && original.get("providerConfigs").is_none_or(Value::is_object)
                && original
                    .pointer("/providerConfigs/codex")
                    .is_none_or(Value::is_object),
            "Claudian 设置结构异常，未覆盖原配置"
        );
        let settings = files.json(&settings_path(&vault))?;
        let providers = object_entry(settings, "providerConfigs");
        let codex = object_entry(providers, "codex");
        // 本机路径不能交给 WSL；不猜测发行版和盘符映射。
        ensure!(
            !codex
                .get("installationMethodsByHost")
                .and_then(Value::as_object)
                .is_some_and(|hosts| hosts.values().any(|value| value.as_str() == Some("wsl"))),
            "Claudian 配置了 WSL Codex，请先改用本机 CLI 后接入"
        );
        let environment = codex
            .get("environmentVariables")
            .and_then(Value::as_str)
            .unwrap_or("");
        codex.insert(
            "environmentVariables".into(),
            json!(with_home(environment, &home)?),
        );
        codex.insert("enabled".into(), json!(true));
        codex.insert(
            "visibleModels".into(),
            json!(wiring.model_ids().collect::<Vec<_>>()),
        );
        // 清除目录指纹，让 Claudian 通过独立 app-server 重新发现，保留旧模型缓存原文供断开恢复。
        codex.insert("catalogTimestamp".into(), json!(0));
        codex.insert("catalogFingerprint".into(), json!(""));
        object_entry(settings, "savedProviderModel")
            .insert("codex".into(), json!(wiring.default_model));
        if settings.get("settingsProvider").and_then(Value::as_str) == Some("codex") {
            settings.insert("model".into(), json!(wiring.default_model));
        }
        let doc = files.toml(&home.join("config.toml"))?;
        doc["model"] = toml_edit::value(&wiring.default_model);
        doc["model_provider"] = toml_edit::value(PROVIDER_ID);
        doc["model_catalog_json"] = toml_edit::value("models.json");
        doc["model_providers"][PROVIDER_ID]["name"] = toml_edit::value("Codex++ Claudian");
        doc["model_providers"][PROVIDER_ID]["base_url"] = toml_edit::value(wiring.v1());
        doc["model_providers"][PROVIDER_ID]["wire_api"] = toml_edit::value("responses");
        doc["model_providers"][PROVIDER_ID]["requires_openai_auth"] = toml_edit::value(false);
        doc["model_providers"][PROVIDER_ID]["experimental_bearer_token"] =
            toml_edit::value(&wiring.token);
        doc["model_providers"][PROVIDER_ID]["supports_websockets"] = toml_edit::value(false);
        let windows = wiring
            .models
            .iter()
            .filter_map(|model| {
                model
                    .context_window
                    .map(|window| (model.id.clone(), window.to_string()))
            })
            .collect();
        let entries = crate::model_suffix::collect_catalog_entries(
            &wiring.model_ids().collect::<Vec<_>>().join("\n"),
            &windows,
            &HashMap::new(),
            &wiring.default_model,
        );
        let mut catalog: Value = serde_json::from_str(
            &crate::model_suffix::build_model_catalog_json(&entries, None),
        )?;
        for entry in catalog["models"]
            .as_array_mut()
            .context("模型目录不是列表")?
        {
            entry["prefer_websockets"] = json!(false);
            entry["tool_mode"] = json!("code_mode_only");
            if let Some(model) = wiring
                .models
                .iter()
                .find(|model| entry["slug"].as_str() == Some(&model.id))
            {
                entry["supported_reasoning_levels"] = json!(
                    model
                        .reasoning_levels
                        .iter()
                        .map(|level| json!({"effort": level, "description": level}))
                        .collect::<Vec<_>>()
                );
                if let Some(level) = model.reasoning_levels.first() {
                    entry["default_reasoning_level"] = json!(level);
                }
            }
        }
        *files.raw(&home.join("models.json"))? = Some(serde_json::to_string_pretty(&catalog)?);
    }
    Ok(())
}

/// 框架只追踪两层键；Claudian 的 Codex 设置在第三层，断开时只还原本适配器改过的字段。
pub(crate) fn restore_settings(
    file: &StashedFile,
    current: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let before = parse_jsonc(file.stash.before.as_deref().unwrap_or("{}"))?;
    let mut restored = parse_jsonc(
        super::files::restored_text(file, current)?
            .as_deref()
            .unwrap_or("{}"),
    )?;
    let mut live = parse_jsonc(current.unwrap_or("{}"))?;
    let old = &before["providerConfigs"]["codex"];
    let codex = live
        .pointer_mut("/providerConfigs/codex")
        .and_then(Value::as_object_mut)
        .context("Claudian Codex 设置已改变结构，停止恢复以免覆盖")?;
    for key in [
        "enabled",
        "visibleModels",
        "catalogTimestamp",
        "catalogFingerprint",
    ] {
        if let Some(value) = old.get(key) {
            codex.insert(key.into(), value.clone());
        } else {
            codex.remove(key);
        }
    }
    let environment = codex
        .get("environmentVariables")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut lines: Vec<_> = environment
        .lines()
        .filter(|line| env_key(line) != Some("CODEX_HOME"))
        .map(str::to_string)
        .collect();
    lines.extend(
        old["environmentVariables"]
            .as_str()
            .unwrap_or("")
            .lines()
            .filter(|line| env_key(line) == Some("CODEX_HOME"))
            .map(str::to_string),
    );
    if old.get("environmentVariables").is_some() || !lines.is_empty() {
        codex.insert("environmentVariables".into(), json!(lines.join("\n")));
    } else {
        codex.remove("environmentVariables");
    }
    if old.is_null() && codex.is_empty() {
        if let Some(providers) = restored
            .get_mut("providerConfigs")
            .and_then(Value::as_object_mut)
        {
            providers.remove("codex");
        }
    } else {
        restored["providerConfigs"]["codex"] = json!(codex);
    }
    if restored == before {
        return Ok(file.stash.before.clone());
    }
    Ok(Some(serde_json::to_string_pretty(&restored)?))
}

#[cfg(test)]
mod tests {
    use super::super::files::AgentStash;
    use super::super::{ModelEntry, Os};
    use super::*;

    fn setup() -> (tempfile::TempDir, AgentEnv, PathBuf, Wiring) {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("中文 Vault");
        std::fs::create_dir_all(vault.join(".claudian")).unwrap();
        std::fs::write(settings_path(&vault), serde_json::to_vec(&json!({
            "providerConfigs":{"codex":{"environmentVariables":"OTHER=keep\nCODEX_HOME=old", "safeMode":"read-only"},
            "claude":{"enabled":true}}, "settingsProvider":"claude", "model":"claude-old"
        })).unwrap()).unwrap();
        let env = AgentEnv::at(temp.path())
            .with_os(Os::Windows)
            .with_var("CLAUDIAN_VAULT", vault.to_str().unwrap());
        let wiring = Wiring {
            agent: "claudian".into(),
            root: "http://127.0.0.1:57321/gw/claudian".into(),
            token: "local-test-token".into(),
            default_model: "api/model".into(),
            small_model: "api/model".into(),
            models: vec![ModelEntry {
                id: "api/model".into(),
                display_name: "API model".into(),
                context_window: Some(200_000),
                reasoning_levels: vec!["high".into()],
            }],
        };
        (temp, env, vault, wiring)
    }

    #[test]
    fn writes_isolated_home_and_preserves_main_login_and_other_backend() {
        let (_temp, env, vault, wiring) = setup();
        let mut files = FileSet::new();
        write(&env, &wiring, &mut files).unwrap();
        let changes = files.changes().unwrap();
        assert_eq!(changes.len(), 3);
        assert!(changes.iter().all(|change| change.path.starts_with(&vault)));
        let settings: Value = serde_json::from_str(
            changes
                .iter()
                .find(|change| change.path == settings_path(&vault))
                .unwrap()
                .after
                .as_ref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(settings["providerConfigs"]["claude"]["enabled"], true);
        assert_eq!(settings["model"], "claude-old");
        let codex = &settings["providerConfigs"]["codex"];
        assert_eq!(codex["safeMode"], "read-only");
        assert!(
            codex["environmentVariables"]
                .as_str()
                .unwrap()
                .contains("OTHER=keep")
        );
        assert_eq!(
            codex["environmentVariables"]
                .as_str()
                .unwrap()
                .matches("CODEX_HOME=")
                .count(),
            1
        );
        let config = changes
            .iter()
            .find(|change| change.path.ends_with("config.toml"))
            .unwrap()
            .after
            .as_ref()
            .unwrap();
        assert!(config.contains("/gw/claudian/v1"));
        assert!(config.contains("requires_openai_auth = false"));
        let catalog: Value = serde_json::from_str(
            changes
                .iter()
                .find(|change| change.path.ends_with("models.json"))
                .unwrap()
                .after
                .as_ref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(catalog["models"][0]["slug"], "api/model");
        assert_eq!(catalog["models"][0]["prefer_websockets"], false);
        assert_eq!(catalog["models"][0]["tool_mode"], "code_mode_only");
    }

    #[test]
    fn disconnect_keeps_new_cli_preferences_and_unrelated_environment() {
        let (_temp, env, vault, wiring) = setup();
        let mut files = FileSet::new();
        write(&env, &wiring, &mut files).unwrap();
        let change = files
            .changes()
            .unwrap()
            .into_iter()
            .find(|change| change.path == settings_path(&vault))
            .unwrap();
        let mut stash = AgentStash::default();
        stash.record(&change).unwrap();
        let mut current: Value = serde_json::from_str(change.after.as_ref().unwrap()).unwrap();
        current["providerConfigs"]["codex"]["safeMode"] = json!("workspace-write");
        current["providerConfigs"]["codex"]["environmentVariables"]
            .as_str()
            .unwrap();
        let env_text = format!(
            "{}\nNEW=value",
            current["providerConfigs"]["codex"]["environmentVariables"]
                .as_str()
                .unwrap()
        );
        current["providerConfigs"]["codex"]["environmentVariables"] = json!(env_text);
        let result = restore_settings(&stash.files[&change.path], Some(&current.to_string()))
            .unwrap()
            .unwrap();
        let restored: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            restored["providerConfigs"]["codex"]["safeMode"],
            "workspace-write"
        );
        let env = restored["providerConfigs"]["codex"]["environmentVariables"]
            .as_str()
            .unwrap();
        assert!(env.contains("NEW=value"));
        assert!(env.contains("CODEX_HOME=old"));
        assert!(!env.contains("/codex-plus"));
    }

    #[test]
    fn registry_discovers_multiple_vaults_deduplicates_and_rejects_wsl_or_malformed_settings() {
        let (temp, _env, vault, wiring) = setup();
        let env = AgentEnv::at(temp.path()).with_os(Os::Windows);
        let second = temp.path().join("第二个 Vault");
        std::fs::create_dir_all(second.join(".claudian")).unwrap();
        std::fs::write(
            settings_path(&second),
            r#"{"providerConfigs":{"codex":{}},"settingsProvider":"codex"}"#,
        )
        .unwrap();
        let registry = env.app_config().join("obsidian/obsidian.json");
        std::fs::create_dir_all(registry.parent().unwrap()).unwrap();
        std::fs::write(
            &registry,
            json!({"vaults":{"one":{"path":vault},"same":{"path":vault},"two":{"path":second}}})
                .to_string(),
        )
        .unwrap();
        assert_eq!(vaults(&env).unwrap().len(), 2);
        let mut files = FileSet::new();
        write(&env, &wiring, &mut files).unwrap();
        assert_eq!(files.changes().unwrap().len(), 6);
        for text in [
            r#"{"providerConfigs":{"codex":{"installationMethodsByHost":{"PC":"wsl"}}}}"#,
            r#"{"providerConfigs":42}"#,
        ] {
            std::fs::write(settings_path(&second), text).unwrap();
            assert!(write(&env, &wiring, &mut FileSet::new()).is_err());
            assert_eq!(
                std::fs::read_to_string(settings_path(&second)).unwrap(),
                text
            );
        }
    }
}
