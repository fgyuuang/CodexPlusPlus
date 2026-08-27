use codex_plus_core::assets::injection_script_with_settings;
use codex_plus_core::protocol_proxy::{
    capacity_retry_status, finish_capacity_retry_notice, record_capacity_retry_notice,
};
use codex_plus_core::settings::{BackendSettings, SettingsStore};

#[test]
fn capacity_retry_defaults_to_false_and_round_trips_through_json() {
    let mut settings = BackendSettings::default();
    assert!(!settings.codex_app_capacity_retry);
    assert_eq!(settings.codex_app_capacity_retry_max_attempts, 5);

    settings.codex_app_capacity_retry = true;
    settings.codex_app_capacity_retry_max_attempts = 7;
    let json = serde_json::to_value(&settings).expect("serialize settings");
    assert_eq!(
        json.get("codexAppCapacityRetry")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        json.get("codexAppCapacityRetryMaxAttempts")
            .and_then(|value| value.as_u64()),
        Some(7)
    );

    let parsed: BackendSettings = serde_json::from_value(json).expect("deserialize settings");
    assert!(parsed.codex_app_capacity_retry);
    assert_eq!(parsed.codex_app_capacity_retry_max_attempts, 7);
}

#[test]
fn capacity_retry_is_preserved_by_partial_settings_updates() {
    let temp = tempfile::tempdir().expect("create temp directory");
    let store = SettingsStore::new(temp.path().join("settings.json"));

    let updated = store
        .update(serde_json::json!({ "codexAppCapacityRetry": true, "codexAppCapacityRetryMaxAttempts": 9 }))
        .expect("update settings");
    assert!(updated.codex_app_capacity_retry);
    assert_eq!(updated.codex_app_capacity_retry_max_attempts, 9);

    let updated = store
        .update(serde_json::json!({ "codexAppThreadIdBadge": true }))
        .expect("update unrelated setting");
    assert!(updated.codex_app_capacity_retry);
    assert_eq!(updated.codex_app_capacity_retry_max_attempts, 9);
}

#[test]
fn injection_script_installs_the_capacity_retry_guard() {
    let script = injection_script_with_settings(0, &BackendSettings::default());

    assert!(script.contains("capacityRetry: \"codexAppCapacityRetry\""));
    assert!(script.contains("capacityRetryMaxAttempts: \"codexAppCapacityRetryMaxAttempts\""));
    assert!(script.contains("installCodexCapacityRetry();"));
    assert!(script.contains("selected model is at capacity"));
    assert!(script.contains("capacity_error_retried"));
    assert!(script.contains("isCodexLocalProtocolProxyRequest"));
    assert!(script.contains("observeCodexCapacityRetryStatus"));
    assert!(script.contains("模型容量不足，Codex++ 正在重试"));
    assert!(script.contains("codexCapacityRetryMaxAttempts"));
    assert!(script.contains("capacity_error_passthrough"));
    assert!(!script.contains("codexCapacityRetrySyntheticResponse"));
    assert!(!script.contains("The upstream service is temporarily unavailable"));
}

#[test]
fn capacity_retry_notice_is_out_of_band_and_tracks_recovery() {
    let sequence = record_capacity_retry_notice(2, 9);
    let retrying = capacity_retry_status();
    assert_eq!(retrying["sequence"], sequence);
    assert_eq!(retrying["phase"], "retrying");
    assert_eq!(retrying["attempt"], 2);
    assert_eq!(retrying["maxAttempts"], 9);

    finish_capacity_retry_notice(sequence, true);
    let recovered = capacity_retry_status();
    assert_eq!(recovered["sequence"], sequence);
    assert_eq!(recovered["phase"], "recovered");
}
