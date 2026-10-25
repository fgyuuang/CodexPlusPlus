use codex_plus_core::official_account_schedule::{quota_error_reset, quota_exhausted};
use codex_plus_core::official_accounts::parse_usage_snapshot;
use serde_json::json;

#[test]
fn quota_windows_require_explicit_exhaustion_and_future_reset() {
    let usage = parse_usage_snapshot(&json!({"rate_limit": {
        "primary_window": {"used_percent": 100, "reset_at": 2000},
        "secondary_window": {"used_percent": 20, "limit_window_seconds": 604800}
    }}));
    assert!(quota_exhausted(Some(&usage), 1000));
    assert!(!quota_exhausted(Some(&usage), 2000));
    assert!(!quota_exhausted(None, 1000));
    let weekly_only = parse_usage_snapshot(&json!({"rate_limit": {
        "primary_window": {"used_percent": 20, "limit_window_seconds": 604800}
    }}));
    assert!(!quota_exhausted(Some(&weekly_only), 1000));
}

#[test]
fn only_structured_official_quota_errors_allow_switching() {
    assert_eq!(
        quota_error_reset(
            br#"{"error":{"type":"usage_limit_reached","resets_at":2000}}"#,
            1000
        ),
        Some(2000)
    );
    for body in [
        br#"{"error":{"code":"rate_limit_exceeded","message":"429"}}"#.as_slice(),
        br#"{"error":{"code":"invalid_api_key"}}"#,
        br#"{"error":{"message":"usage_limit_reached"}}"#,
        br#"{"input":"usage_limit_reached"}"#,
    ] {
        assert_eq!(quota_error_reset(body, 1000), None);
    }
    assert_eq!(quota_error_reset(b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_limit_reached\"}}}\n\n", 1000), Some(1300));
    // 已产生输出的流绝不能重放，即使之后出现额度错误。
    assert_eq!(quota_error_reset(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\ndata: {\"error\":{\"type\":\"usage_limit_reached\"}}\n\n", 1000), None);
}

#[test]
fn fragmented_stream_quota_error_is_observed_after_output_without_replaying_it() {
    let mut tracker = codex_plus_core::protocol_proxy::ResponsesSseTerminalTracker::default();
    tracker.observe(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n");
    tracker.observe(b"event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"usage_limit_");
    tracker.observe(b"reached\"}}}\n\n");
    assert!(tracker.has_failed());
    assert!(tracker.quota_reset.is_some());
}
