use codex_plus_core::assets::injection_script_with_settings;
use codex_plus_core::settings::{BackendSettings, SettingsStore};
use serde_json::{Value, json};

const LOCAL_FEATURE_KEYS: &[&str] = &[
    "codexAppFastStartup",
    "codexAppNativeMenuLocalization",
    "codexAppZedRemoteOpen",
    "zedRemoteOpenStrategy",
    "zedRemoteProjectRegistryEnabled",
    "zedRemoteSyncToZedSettings",
    "codexAppUpstreamWorktreeCreate",
];

fn old_settings() -> Value {
    json!({
        "codexAppFastStartup": true,
        "codexAppForceChineseLocale": true, "codexAppNativeMenuLocalization": true,
        "codexAppZedRemoteOpen": true, "zedRemoteOpenStrategy": "addToFocusedWorkspace",
        "zedRemoteProjectRegistryEnabled": true, "zedRemoteSyncToZedSettings": true,
        "codexAppUpstreamWorktreeCreate": true, "codexAppNativeMenuPlacement": true,
        "codexAppStepwiseEnabled": true, "dictation": {"enabled": true},
        "preservedUnknownSetting": "keep",
    })
}

#[test]
fn local_features_are_preserved_when_loading_old_settings() {
    let settings: BackendSettings = serde_json::from_value(old_settings()).unwrap();
    let serialized = serde_json::to_value(&settings).unwrap();
    for key in LOCAL_FEATURE_KEYS {
        assert_eq!(serialized.get(*key), old_settings().get(*key), "{key}");
    }
    assert!(settings.codex_app_native_menu_placement);
    assert!(settings.codex_app_stepwise_enabled);
    assert!(settings.dictation.enabled);
}

#[test]
fn partial_updates_preserve_local_features_and_unrelated_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, serde_json::to_vec(&old_settings()).unwrap()).unwrap();
    let store = SettingsStore::new(path.clone());
    let updated = store
        .update(json!({ "codexAppStepwiseEnabled": false }))
        .unwrap();
    let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for key in LOCAL_FEATURE_KEYS {
        assert_eq!(raw.get(*key), old_settings().get(*key), "{key}");
    }
    assert_eq!(raw["preservedUnknownSetting"], "keep");
    assert!(updated.codex_app_native_menu_placement);
    assert!(!updated.codex_app_stepwise_enabled);
    assert!(updated.dictation.enabled);
    let updated = store
        .update(json!({
            "codexAppFastStartup": false,
            "codexAppNativeMenuLocalization": false,
            "codexAppZedRemoteOpen": false,
        }))
        .unwrap();
    assert!(!updated.codex_app_fast_startup);
    assert!(!updated.codex_app_native_menu_localization);
    assert!(!updated.codex_app_zed_remote_open);
    assert!(updated.codex_app_upstream_worktree_create);
    let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(raw["preservedUnknownSetting"], "keep");
    assert_eq!(raw["dictation"]["enabled"], true);
    assert_eq!(raw["codexAppStepwiseEnabled"], false);
}

#[test]
fn force_chinese_locale_defaults_to_true_and_fast_startup_defaults_off() {
    let settings = BackendSettings::default();
    assert!(settings.codex_app_force_chinese_locale);

    let json = serde_json::to_value(&settings).expect("serialize default settings");
    assert_eq!(
        json.get("codexAppForceChineseLocale")
            .and_then(|v| v.as_bool()),
        Some(true),
        "local BackendSettings JSON retains codexAppForceChineseLocale = true"
    );
    assert_eq!(json["codexAppFastStartup"], false);
}

#[test]
fn force_chinese_locale_missing_from_old_json_preserves_local_default() {
    let json = serde_json::json!({
        "codexAppPath": "",
        "enhancementsEnabled": true,
    });

    let parsed: BackendSettings = serde_json::from_value(json)
        .expect("old settings JSON without codexAppForceChineseLocale should still load");
    assert!(
        parsed.codex_app_force_chinese_locale,
        "缺键沿用本地默认中文设置，显式 false 由后续回归独立验证"
    );
}

#[test]
fn force_chinese_locale_explicit_false_round_trips_through_json() {
    let mut settings = BackendSettings::default();
    settings.codex_app_force_chinese_locale = false;

    let json = serde_json::to_value(&settings).expect("serialize");
    assert_eq!(
        json.get("codexAppForceChineseLocale")
            .and_then(|v| v.as_bool()),
        Some(false)
    );

    let parsed: BackendSettings =
        serde_json::from_value(json).expect("deserialize codexAppForceChineseLocale");
    assert!(!parsed.codex_app_force_chinese_locale);
}

#[test]
fn force_chinese_locale_config_follows_the_switch() {
    let mut settings = BackendSettings::default();
    settings.codex_app_force_chinese_locale = true;
    settings.codex_app_fast_startup = true;
    let script = injection_script_with_settings(0, &settings);
    assert!(script.contains(
        "window.__CODEX_PLUS_FORCE_CHINESE_LOCALE__ = {\"enabled\":true,\"locale\":\"zh-CN\"};"
    ));
    assert!(script.contains(
        "window.__CODEX_PLUS_FAST_STARTUP__ = {\"enabled\":true,\"statsigTimeoutMs\":800};"
    ));
    assert!(script.contains("__codexPlusForceChineseLocaleInstalled"));
    assert!(script.contains("__codexPlusFastStartupInstalled"));
    assert!(script.contains("72216192"));
    assert!(script.contains("enable_i18n"));
    assert!(script.contains("locale_source"));
    assert!(script.contains("vscode://codex/${method}"));
    assert!(script.contains("\"get-setting\""));
    assert!(script.contains("\"set-setting\""));
    assert!(script.contains("{ key: \"localeOverride\", value: locale }"));
    assert!(script.contains("window.location.reload()"));
    assert!(
        script.contains(
            "if (window.sessionStorage.getItem(localeReloadStorageKey) !== marker) return;"
        )
    );
    assert!(script.contains("codexPlus.forceChineseLocale.managed.v1"));
    assert!(!script.contains("setItem(\"localeOverride\""));
    assert_eq!(
        codex_plus_core::assets::force_chinese_locale_config(&settings),
        json!({ "enabled": true, "locale": "zh-CN" })
    );

    settings.codex_app_force_chinese_locale = false;
    assert_eq!(
        codex_plus_core::assets::force_chinese_locale_config(&settings),
        json!({ "enabled": false, "locale": "zh-CN" })
    );
}

#[test]
fn force_chinese_locale_and_fast_startup_are_independent_injection_controls() {
    let mut settings: BackendSettings = serde_json::from_value(old_settings()).unwrap();
    let script = injection_script_with_settings(0, &settings);
    assert!(script.contains(
        "window.__CODEX_PLUS_FORCE_CHINESE_LOCALE__ = {\"enabled\":true,\"locale\":\"zh-CN\"}"
    ));
    assert!(script.contains("installCodexPlusForceChineseLocale"));
    // 旧中文清理 shim 保持撤下，已发布的快速启动开关继续独立工作。
    assert!(!script.contains("restoreCodexPlusManagedLocale"));
    assert!(script.contains(
        "window.__CODEX_PLUS_FAST_STARTUP__ = {\"enabled\":true,\"statsigTimeoutMs\":800};"
    ));
    assert!(script.contains("installCodexPlusFastStartup();"));
    assert!(script.contains("__codexPlusFastStartupInstalled"));
    settings.codex_app_fast_startup = false;
    let script = injection_script_with_settings(0, &settings);
    assert!(script.contains(
        "window.__CODEX_PLUS_FAST_STARTUP__ = {\"enabled\":false,\"statsigTimeoutMs\":800};"
    ));
    assert!(script.contains(
        "window.__CODEX_PLUS_FORCE_CHINESE_LOCALE__ = {\"enabled\":true,\"locale\":\"zh-CN\"};"
    ));
}
