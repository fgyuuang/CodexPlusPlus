// 受保护本地来源的HTTP/SSE生命周期，保留账号归属、容量重试、终止事件和渠道permit。

use super::*;
use std::time::Duration;

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[tokio::test]
    async fn protected_handler_records_usage_once_without_gateway_or_user_state() {
        let _logs = crate::request_log::request_log_test_guard();
        let _settings = crate::paths::settings_path_test_guard();
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("request-log.jsonl");
        let settings = temp.path().join("settings.json");
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = upstream.local_addr().unwrap();
        std::fs::write(
            &settings,
            serde_json::to_vec(&serde_json::json!({
                "relayProfilesEnabled":true, "activeRelayId":"test", "relayProfiles":[{
                    "id":"test", "name":"isolated", "relayMode":"pureApi", "protocol":"responses",
                    "baseUrl":format!("http://{address}/v1"), "apiKey":"test-key"
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let previous = crate::paths::set_settings_path_for_tests(Some(settings));
        crate::request_log::set_request_log_path_for_tests(Some(log.clone()));
        let producer = tokio::spawn(async move {
            let (mut socket, _) = upstream.accept().await.unwrap();
            let mut bytes = [0u8; 8192];
            let _ = socket.read(&mut bytes).await.unwrap();
            let body = "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1200,\"output_tokens\":100}}}\n\n";
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
            socket.write_all(body.as_bytes()).await.unwrap();
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handler = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            handle_protocol_proxy_connection(
                &mut socket,
                r#"{"model":"gpt-6.1-sol","stream":true,"input":"isolated test"}"#,
                None,
                None,
                Default::default(),
                "POST",
                "/v1/responses",
                None,
            )
            .await
            .unwrap();
        });
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        let mut body = Vec::new();
        client.read_to_end(&mut body).await.unwrap();
        handler.await.unwrap();
        producer.await.unwrap();
        let entries = crate::request_log::read_request_logs_at(&log, 10).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].success);
        assert_eq!(entries[0].usage.as_ref().unwrap().input_tokens, 1200);
        assert_eq!(entries[0].usage.as_ref().unwrap().output_tokens, 100);
        let usage = temp.path().join("gateway-usage.json");
        tokio::time::timeout(Duration::from_secs(2), async {
            while !usage.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(&usage).unwrap()).unwrap();
        assert_eq!(
            value["days"].as_object().unwrap().values().next().unwrap()[0]["inputTokens"],
            1200
        );
        crate::request_log::set_request_log_path_for_tests(None);
        crate::paths::set_settings_path_for_tests(previous);
    }
}

pub(super) async fn handle_official_images_proxy_connection(
    stream: &mut tokio::net::TcpStream,
    request_body: &str,
    operation: crate::protocol_proxy::protected_local::ImageProxyOperation,
    request_user_agent: Option<&str>,
    session_headers: crate::protocol_proxy::protected_local::ProxySessionHeaders<'_>,
    method: &str,
    path: &str,
    remote_addr_text: Option<String>,
) -> anyhow::Result<()> {
    let upstream = match crate::protocol_proxy::protected_local::open_images_proxy_request_for_path_with_session(
        request_body,
        operation,
        request_user_agent,
        path,
        session_headers,
    )
    .await
    {
        Ok(upstream) => upstream,
        Err(error) => {
            let body = serde_json::to_vec(&serde_json::json!({
                "status": "failed",
                "message": error.to_string()
            }))?;
            write_http_response(
                stream,
                "502 Bad Gateway",
                "application/json; charset=utf-8",
                &body,
            )
            .await?;
            log_helper_response(
                "helper.official_image_proxy_failed",
                method,
                path,
                "502 Bad Gateway",
                remote_addr_text,
            );
            stream.shutdown().await?;
            return Ok(());
        }
    };
    let status = upstream.status();
    let is_success = upstream.is_success();
    let content_type = if upstream.content_type.is_empty() {
        "application/json; charset=utf-8".to_string()
    } else {
        upstream.content_type.clone()
    };
    let body = crate::protocol_proxy::protected_local::read_upstream_body_with_timeout(
        upstream.response,
        crate::protocol_proxy::protected_local::upstream_image_body_timeout(),
    )
    .await?;
    write_http_response(stream, &status, &content_type, &body).await?;
    log_helper_response(
        if is_success {
            "helper.official_image_proxy_ok"
        } else {
            "helper.official_image_proxy_upstream_error"
        },
        method,
        path,
        &status,
        remote_addr_text,
    );
    stream.shutdown().await?;
    Ok(())
}

pub(super) async fn handle_models_proxy_connection(
    stream: &mut tokio::net::TcpStream,
    request_user_agent: Option<&str>,
    method: &str,
    path: &str,
    remote_addr_text: Option<String>,
) -> anyhow::Result<()> {
    if method == "OPTIONS" {
        write_http_response(
            stream,
            "204 No Content",
            "application/json; charset=utf-8",
            &[],
        )
        .await?;
        stream.shutdown().await?;
        return Ok(());
    }
    let upstream = match crate::protocol_proxy::protected_local::open_models_proxy_request(
        request_user_agent,
    )
    .await
    {
        Ok(upstream) => upstream,
        Err(error) => {
            let body = serde_json::to_vec(
                &serde_json::json!({                 "status": "failed",                 "message": error.to_string()             }),
            )?;
            write_http_response(
                stream,
                "502 Bad Gateway",
                "application/json; charset=utf-8",
                &body,
            )
            .await?;
            log_helper_proxy_failure(
                "helper.models_proxy_failed",
                method,
                path,
                &error.to_string(),
                remote_addr_text,
            );
            stream.shutdown().await?;
            return Ok(());
        }
    };
    let status = upstream.status();
    let is_success = upstream.is_success();
    let content_type = if upstream.content_type.is_empty() {
        "application/json; charset=utf-8".to_string()
    } else {
        upstream.content_type.clone()
    };
    let body = crate::protocol_proxy::protected_local::read_upstream_body_with_timeout(
        upstream.response,
        crate::protocol_proxy::protected_local::upstream_body_timeout(),
    )
    .await?;
    write_http_response(stream, &status, &content_type, &body).await?;
    log_helper_response(
        if is_success {
            "helper.models_proxy_ok"
        } else {
            "helper.models_proxy_upstream_error"
        },
        method,
        path,
        &status,
        remote_addr_text,
    );
    stream.shutdown().await?;
    Ok(())
}

pub(super) async fn handle_protocol_proxy_connection(
    stream: &mut tokio::net::TcpStream,
    request_body: &str,
    request_user_agent: Option<&str>,
    request_beta_features: Option<&str>,
    session_headers: crate::protocol_proxy::protected_local::ProxySessionHeaders<'_>,
    method: &str,
    path: &str,
    remote_addr_text: Option<String>,
) -> anyhow::Result<()> {
    let request_json = serde_json::from_str::<serde_json::Value>(request_body).ok();
    // 受保护的官方路径过去提前分流，绕过了主代理的用量日志。
    let mut request_log = ProxyRequestLog::begin(
        method,
        path,
        request_json.as_ref(),
        session_headers
            .thread_id
            .or(session_headers.session_id)
            .map(str::to_string),
    );

    let request_is_stream = request_json
        .as_ref()
        .and_then(|request| request.get("stream"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let mut capacity_retry_round = 0u8;
    let mut capacity_retry_notice_sequence = None;
    let upstream = loop {
        let upstream = match crate::request_log::LIVE_REQUEST_ID.scope(request_log.entry.id.clone(), Box::pin(crate::protocol_proxy::protected_local::open_responses_proxy_request_with_capacity_retries_for_path_and_session_headers(
            request_body,
            request_user_agent,
            path,
            request_beta_features,
            session_headers,
            )))
            .await
            {
                Ok(upstream) => upstream,
                Err(error) => {
                    if let Some(sequence) = capacity_retry_notice_sequence {
                        crate::protocol_proxy::protected_local::finish_capacity_retry_notice(sequence, false);
                    }
                    if request_is_stream {
                        let error_text = error.to_string();
                        let body = crate::protocol_proxy::protected_local::responses_stream_failure_from_upstream(
                            request_json.as_ref(),
                            502,
                            "text/plain; charset=utf-8",
                            error_text.as_bytes(),
                        );
                        write_http_response(
                            stream,
                            "200 OK",
                            "text/event-stream; charset=utf-8",
                            &body,
                        )
                        .await?;
                        log_helper_response(
                            "helper.protocol_proxy_stream_failed",
                            method,
                            path,
                            "502 Bad Gateway",
                            remote_addr_text,
                        );
                        stream.shutdown().await?;
                        return Ok(());
                    }
                    let body = serde_json::to_vec(
                        &serde_json::json!({                     "status": "failed",                     "message": error.to_string()                 }),
                    )?;
                    write_http_response(
                        stream,
                        "502 Bad Gateway",
                        "application/json; charset=utf-8",
                        &body,
                    )
                    .await?;
                    log_helper_response(
                        "helper.protocol_proxy_failed",
                        method,
                        path,
                        "502 Bad Gateway",
                        remote_addr_text,
                    );
                    stream.shutdown().await?;
                    return Ok(());
                }
            };

        if upstream.capacity_retryable {
            capacity_retry_round = capacity_retry_round.saturating_add(1);
            capacity_retry_notice_sequence = Some(
                crate::protocol_proxy::protected_local::record_capacity_retry_notice(
                    capacity_retry_round,
                    upstream.capacity_retry_max_attempts,
                ),
            );
            let delay = Duration::from_millis(
                (250u64 * u64::from(capacity_retry_round.clamp(1, 8))).min(2_000),
            );
            tokio::time::sleep(delay).await;
            continue;
        }
        if upstream.is_success() {
            if let Some(sequence) = capacity_retry_notice_sequence {
                crate::protocol_proxy::protected_local::finish_capacity_retry_notice(
                    sequence, true,
                );
            }
            break upstream;
        }

        let status = upstream.status();
        let capacity_retry_enabled = upstream.capacity_retry_enabled;
        let capacity_retry_key = upstream.capacity_retry_key;
        let capacity_retry_max_attempts = upstream.capacity_retry_max_attempts;
        let upstream_content_type = upstream.content_type.clone();
        let mut upstream_body = upstream.prefetched_chunk;
        let error_body = crate::protocol_proxy::protected_local::read_upstream_body_with_timeout(
            upstream.response,
            crate::protocol_proxy::protected_local::upstream_body_timeout(),
        )
        .await;
        match error_body {
            Ok(body) => upstream_body.extend_from_slice(&body),
            Err(error) if request_is_stream => {
                let failed =
                    crate::protocol_proxy::protected_local::responses_stream_failure_events(
                        request_json.as_ref(),
                        format!("Upstream error body unavailable: {error}"),
                        Some("stream_error".to_string()),
                    );
                write_http_response(
                    stream,
                    "200 OK",
                    "text/event-stream; charset=utf-8",
                    &failed,
                )
                .await?;
                log_helper_response(
                    "helper.protocol_proxy_stream_failed",
                    method,
                    path,
                    &status,
                    remote_addr_text,
                );
                stream.shutdown().await?;
                return Ok(());
            }
            Err(error) => return Err(error),
        }
        let is_capacity = capacity_retry_enabled
            && crate::protocol_proxy::protected_local::is_selected_model_capacity_error(
                &upstream_body,
            );

        if is_capacity
            && capacity_retry_key.is_some_and(|key| {
                crate::protocol_proxy::protected_local::next_capacity_retry_attempt(
                    key,
                    capacity_retry_max_attempts,
                )
                .is_some()
            })
        {
            capacity_retry_round = capacity_retry_round.saturating_add(1);
            capacity_retry_notice_sequence = Some(
                crate::protocol_proxy::protected_local::record_capacity_retry_notice(
                    capacity_retry_round,
                    capacity_retry_max_attempts,
                ),
            );
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "protocol_proxy.capacity_retry_loop",
                serde_json::json!({
                    "source": "launcher",
                    "attempt": capacity_retry_round,
                    "maxAttempts": capacity_retry_max_attempts,
                    "willRetry": true,
                    "reason": "selected_model_at_capacity"
                }),
            );
            let delay = Duration::from_millis(
                (250u64 * u64::from(capacity_retry_round.clamp(1, 8))).min(2_000),
            );
            tokio::time::sleep(delay).await;
            continue;
        }
        if !is_capacity {
            if let Some(key) = capacity_retry_key {
                crate::protocol_proxy::protected_local::reset_capacity_retry_attempts(key);
            }
            if let Some(sequence) = capacity_retry_notice_sequence {
                crate::protocol_proxy::protected_local::finish_capacity_retry_notice(
                    sequence, false,
                );
            }
        } else if let Some(sequence) = capacity_retry_notice_sequence {
            crate::protocol_proxy::protected_local::finish_capacity_retry_notice(sequence, false);
        }

        if request_is_stream {
            let body =
                crate::protocol_proxy::protected_local::responses_stream_failure_from_upstream(
                    request_json.as_ref(),
                    upstream.status_code,
                    &upstream_content_type,
                    &upstream_body,
                );
            write_http_response(stream, "200 OK", "text/event-stream; charset=utf-8", &body)
                .await?;
            log_helper_response(
                "helper.protocol_proxy_stream_failed",
                method,
                path,
                &status,
                remote_addr_text,
            );
            stream.shutdown().await?;
            return Ok(());
        }
        let error = crate::protocol_proxy::protected_local::responses_error_from_upstream(
            upstream.status_code,
            &upstream_content_type,
            &upstream_body,
        );
        let body = serde_json::to_vec(&error)?;
        write_http_response(stream, &status, "application/json; charset=utf-8", &body).await?;
        log_helper_response(
            "helper.protocol_proxy_upstream_error",
            method,
            path,
            &status,
            remote_addr_text,
        );
        stream.shutdown().await?;
        return Ok(());
    };
    request_log.entry.status_code = upstream.status_code;
    request_log.entry.stream = upstream.is_stream;
    if let Some(trace) = &upstream.request_trace {
        request_log.entry.relay_id = trace.relay_id.clone();
        request_log.entry.relay_name = trace.relay_name.clone();
        request_log.entry.upstream_model = trace.upstream_model.clone();
        request_log.entry.wire_api = trace.wire_api.clone();
        request_log.entry.attempts = trace.attempts;
    }
    request_log.update_running();
    // 读取或写入中断时 Drop 保持失败，不把仅收到响应头当作成功。
    if upstream.wire_api
        == crate::protocol_proxy::protected_local::UpstreamWireApi::AnthropicMessages
    {
        // 新增协议仅转换供应商数据，账号选择与permit寿命仍由本地路径负责。
        if upstream.is_stream {
            write_http_stream_headers(stream, "200 OK", "text/event-stream; charset=utf-8").await?;
            let wire = crate::protocol_proxy::UpstreamWireApi::AnthropicMessages;
            let (failure, usage, detail, _) = super::forward_protocol_proxy_stream(
                stream,
                &upstream.prefetched_chunk,
                upstream.response,
                wire,
                request_json.as_ref(),
                session_headers.thread_id,
            )
            .await?;
            request_log.record_stream(failure.as_deref(), usage, detail);
            log_helper_response(
                if failure.is_some() {
                    "helper.protocol_proxy_stream_failed"
                } else {
                    "helper.protocol_proxy_stream_ok"
                },
                method,
                path,
                "200 OK",
                remote_addr_text,
            );
        } else {
            let raw = crate::protocol_proxy::protected_local::read_upstream_body_with_timeout(
                upstream.response,
                crate::protocol_proxy::upstream_body_timeout(),
            )
            .await?;
            let message: Value = serde_json::from_slice(&raw)?;
            let thinking =
                crate::anthropic_bridge::thinking_blocks_from_anthropic_message(&message);
            let chat = crate::anthropic_bridge::anthropic_message_to_chat_completion(message)?;
            let mut response = if let Some(request) = &request_json {
                crate::protocol_proxy::chat_completion_to_response_with_request(chat, request)?
            } else {
                crate::protocol_proxy::chat_completion_to_response(chat)?
            };
            crate::anthropic_bridge::attach_thinking_to_responses(&mut response, &thinking);
            request_log.record_usage_json(&serde_json::to_vec(&response)?);
            write_http_response(
                stream,
                "200 OK",
                "application/json; charset=utf-8",
                &serde_json::to_vec(&response)?,
            )
            .await?;
            request_log.entry.success = true;
            log_helper_response(
                "helper.protocol_proxy_ok",
                method,
                path,
                "200 OK",
                remote_addr_text,
            );
        }
        stream.shutdown().await?;
        return Ok(());
    }
    if upstream.is_stream {
        write_http_stream_headers(stream, "200 OK", "text/event-stream; charset=utf-8").await?;
        if upstream.compaction {
            // v2 远程压缩：无论上游协议都重组为恰好一个 compaction 输出项，
            // 压缩无增量展示诉求，收齐上游文本后一次性下发。
            // SSE 事件可能跨网络 chunk 拆开，converter 内部按事件边界缓冲。
            let mut converter = crate::protocol_proxy::protected_local::CompactionSseConverter::new(
                request_json
                    .as_ref()
                    .and_then(|request| request.get("model"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(""),
            );
            if upstream.wire_api
                != crate::protocol_proxy::protected_local::UpstreamWireApi::Responses
            {
                converter = converter.with_chat_upstream();
            }
            converter.push_upstream_bytes(&upstream.prefetched_chunk);
            let mut bytes_stream = Box::pin(upstream.response.bytes_stream());
            loop {
                match crate::protocol_proxy::protected_local::next_stream_chunk_with_timeout(
                    bytes_stream.as_mut(),
                    crate::protocol_proxy::protected_local::upstream_stream_idle_timeout(),
                )
                .await
                {
                    Ok(Some(Ok(bytes))) => converter.push_upstream_bytes(&bytes),
                    Ok(Some(Err(error))) => {
                        converter.fail(format!("Stream error: {error}"), None);
                        break;
                    }
                    Ok(None) => break,
                    Err(_) => {
                        converter.fail(
                            "Compaction stream idle timeout".to_string(),
                            Some("stream_idle_timeout".to_string()),
                        );
                        break;
                    }
                }
            }
            if !converter.has_upstream_terminal() {
                converter.fail(
                    "Compaction stream ended before a terminal event".to_string(),
                    Some("incomplete_stream".to_string()),
                );
            }
            let payload = converter.finish();
            let mut terminal =
                crate::protocol_proxy::protected_local::ResponsesSseTerminalTracker::default();
            terminal.observe(&payload);
            stream.write_all(&payload).await?;
            request_log.record_stream(
                terminal.has_failed().then_some("compaction_failed"),
                terminal.usage.take(),
                None,
            );
            log_helper_response(
                if terminal.has_failed() {
                    "helper.protocol_proxy_stream_failed"
                } else {
                    "helper.protocol_proxy_compaction_ok"
                },
                method,
                path,
                "200 OK",
                remote_addr_text,
            );
            stream.shutdown().await?;
            return Ok(());
        }
        if upstream.wire_api == crate::protocol_proxy::protected_local::UpstreamWireApi::Responses {
            let scheduled_account_id = upstream.scheduled_account_id.clone();
            let prefetched_chunk = upstream.prefetched_chunk;
            let mut bytes_stream = Box::pin(upstream.response.bytes_stream());
            let mut terminal =
                crate::protocol_proxy::protected_local::ResponsesSseTerminalTracker::default();
            let mut stream_failed = false;
            if !prefetched_chunk.is_empty() {
                terminal.observe(&prefetched_chunk);
                stream.write_all(&prefetched_chunk).await?;
            }
            loop {
                match crate::protocol_proxy::protected_local::next_stream_chunk_with_timeout(
                    bytes_stream.as_mut(),
                    crate::protocol_proxy::protected_local::upstream_stream_idle_timeout(),
                )
                .await
                {
                    Ok(Some(Ok(bytes))) => {
                        terminal.observe(&bytes);
                        stream.write_all(&bytes).await?;
                    }
                    Ok(Some(Err(error))) => {
                        let failed =
                            crate::protocol_proxy::protected_local::responses_stream_failure_events(
                                request_json.as_ref(),
                                format!("Stream error: {error}"),
                                Some("stream_error".to_string()),
                            );
                        stream.write_all(&failed).await?;
                        stream_failed = true;
                        break;
                    }
                    Ok(None) => break,
                    Err(
                        crate::protocol_proxy::protected_local::StreamChunkWaitError::IdleTimeout,
                    ) => {
                        let idle_timeout =
                            crate::protocol_proxy::protected_local::upstream_stream_idle_timeout()
                                .as_secs();
                        let failed =
                            crate::protocol_proxy::protected_local::responses_stream_failure_events(
                                request_json.as_ref(),
                                format!("Upstream stream idle for {idle_timeout} seconds"),
                                Some("stream_idle_timeout".to_string()),
                            );
                        stream.write_all(&failed).await?;
                        let _ = crate::diagnostic_log::append_diagnostic_log(
                            "helper.protocol_proxy_stream_idle_timeout",
                            serde_json::json!({
                                "method": method,
                                "path": path,
                                "idle_timeout_seconds": idle_timeout,
                                "remote_addr": remote_addr_text.clone(),
                            }),
                        );
                        stream_failed = true;
                        break;
                    }
                }
            }
            if !stream_failed {
                terminal.finish();
                if !terminal.is_terminal() {
                    let failed =
                        crate::protocol_proxy::protected_local::responses_stream_failure_events(
                            request_json.as_ref(),
                            "Upstream stream ended before a terminal Responses event".to_string(),
                            Some("incomplete_stream".to_string()),
                        );
                    stream.write_all(&failed).await?;
                    stream_failed = true;
                }
            }
            stream_failed |= terminal.has_failed();
            request_log.record_stream(
                stream_failed.then_some("stream_failed"),
                terminal.usage.take(),
                None,
            );
            if let (Some(account_id), Some(until)) = (scheduled_account_id, terminal.quota_reset) {
                crate::official_account_schedule::mark_exhausted(&account_id, until);
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "protocol_proxy.official_schedule_quota_exhausted",
                    serde_json::json!({"sourceId": account_id, "resetsAt": until, "phase": "stream_terminal"}),
                );
            }
            log_helper_response(
                if stream_failed {
                    "helper.protocol_proxy_stream_failed"
                } else {
                    "helper.protocol_proxy_stream_ok"
                },
                method,
                path,
                "200 OK",
                remote_addr_text,
            );
            stream.shutdown().await?;
            return Ok(());
        }
        let mut converter = request_json
            .as_ref()
            .map(crate::protocol_proxy::protected_local::ChatSseToResponsesConverter::with_request)
            .unwrap_or_default();
        let prefetched_chunk = upstream.prefetched_chunk;
        let mut bytes_stream = Box::pin(upstream.response.bytes_stream());
        let mut stream_failed = false;
        if !prefetched_chunk.is_empty() {
            let converted = converter.push_bytes(&prefetched_chunk);
            if !converted.is_empty() {
                stream.write_all(&converted).await?;
            }
        }
        loop {
            match crate::protocol_proxy::protected_local::next_stream_chunk_with_timeout(
                bytes_stream.as_mut(),
                crate::protocol_proxy::protected_local::upstream_stream_idle_timeout(),
            )
            .await
            {
                Ok(Some(Ok(bytes))) => {
                    let converted = converter.push_bytes(&bytes);
                    if !converted.is_empty() {
                        stream.write_all(&converted).await?;
                    }
                }
                Ok(Some(Err(error))) => {
                    let failed = converter.fail(
                        format!("Stream error: {error}"),
                        Some("stream_error".to_string()),
                    );
                    if !failed.is_empty() {
                        stream.write_all(&failed).await?;
                    }
                    stream_failed = true;
                    break;
                }
                Ok(None) => break,
                Err(crate::protocol_proxy::protected_local::StreamChunkWaitError::IdleTimeout) => {
                    let idle_timeout =
                        crate::protocol_proxy::protected_local::upstream_stream_idle_timeout()
                            .as_secs();
                    let failed = converter.fail(
                        format!("Upstream stream idle for {idle_timeout} seconds"),
                        Some("stream_idle_timeout".to_string()),
                    );
                    if !failed.is_empty() {
                        stream.write_all(&failed).await?;
                    }
                    let _ = crate::diagnostic_log::append_diagnostic_log(
                        "helper.protocol_proxy_stream_idle_timeout",
                        serde_json::json!({
                            "method": method,
                            "path": path,
                            "idle_timeout_seconds": idle_timeout,
                            "remote_addr": remote_addr_text.clone(),
                        }),
                    );
                    stream_failed = true;
                    break;
                }
            }
        }
        if !stream_failed {
            let tail = converter.finish();
            if !tail.is_empty() {
                stream.write_all(&tail).await?;
            }
        }
        stream_failed |= converter.has_failed();
        request_log.record_stream(stream_failed.then_some("stream_failed"), None, None);
        log_helper_response(
            if stream_failed {
                "helper.protocol_proxy_stream_failed"
            } else {
                "helper.protocol_proxy_stream_ok"
            },
            method,
            path,
            "200 OK",
            remote_addr_text,
        );
        stream.shutdown().await?;
        return Ok(());
    }
    let upstream_body = crate::protocol_proxy::protected_local::read_upstream_body_with_timeout(
        upstream.response,
        crate::protocol_proxy::protected_local::upstream_body_timeout(),
    )
    .await?;
    request_log.record_usage_json(&upstream_body);
    if upstream.compaction {
        let body = crate::protocol_proxy::protected_local::wrap_non_stream_response_as_compaction(
            &upstream_body,
            request_json
                .as_ref()
                .and_then(|request| request.get("model"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(""),
        )?;
        write_http_response(stream, "200 OK", "text/event-stream; charset=utf-8", &body).await?;
        request_log.entry.success = true;
        log_helper_response(
            "helper.protocol_proxy_compaction_ok",
            method,
            path,
            "200 OK",
            remote_addr_text,
        );
        stream.shutdown().await?;
        return Ok(());
    }

    if upstream.wire_api == crate::protocol_proxy::protected_local::UpstreamWireApi::Responses {
        write_http_response(
            stream,
            "200 OK",
            if upstream.content_type.is_empty() {
                "application/json; charset=utf-8"
            } else {
                &upstream.content_type
            },
            &upstream_body,
        )
        .await?;
        log_helper_response(
            "helper.protocol_proxy_ok",
            method,
            path,
            "200 OK",
            remote_addr_text,
        );
        request_log.entry.success = true;
        stream.shutdown().await?;
        return Ok(());
    }
    let chat_json: serde_json::Value = serde_json::from_slice(&upstream_body)?;
    let response_json = if let Some(request_json) = request_json.as_ref() {
        crate::protocol_proxy::protected_local::chat_completion_to_response_with_request(
            chat_json,
            request_json,
        )?
    } else {
        crate::protocol_proxy::protected_local::chat_completion_to_response(chat_json)?
    };
    let body = serde_json::to_vec(&response_json)?;
    write_http_response(stream, "200 OK", "application/json; charset=utf-8", &body).await?;
    request_log.entry.success = true;
    log_helper_response(
        "helper.protocol_proxy_ok",
        method,
        path,
        "200 OK",
        remote_addr_text,
    );
    stream.shutdown().await?;
    Ok(())
}

pub(super) async fn handle_audio_transcriptions_proxy_connection(
    stream: &mut tokio::net::TcpStream,
    request_body: &[u8],
    request_content_type: Option<&str>,
    request_user_agent: Option<&str>,
    method: &str,
    path: &str,
    remote_addr_text: Option<String>,
) -> anyhow::Result<()> {
    let upstream =
        match crate::protocol_proxy::protected_local::open_audio_transcriptions_proxy_request(
            request_body,
            request_content_type.unwrap_or_default(),
            request_user_agent,
        )
        .await
        {
            Ok(upstream) => upstream,
            Err(error) => {
                let body = serde_json::to_vec(&serde_json::json!({
                    "status": "failed",
                    "message": error.to_string()
                }))?;
                write_http_response(
                    stream,
                    "502 Bad Gateway",
                    "application/json; charset=utf-8",
                    &body,
                )
                .await?;
                log_helper_proxy_failure(
                    "helper.audio_transcriptions_proxy_failed",
                    method,
                    path,
                    &error.to_string(),
                    remote_addr_text,
                );
                stream.shutdown().await?;
                return Ok(());
            }
        };
    let status = upstream.status();
    let is_success = upstream.is_success();
    let content_type = if upstream.content_type.is_empty() {
        "application/json; charset=utf-8".to_string()
    } else {
        upstream.content_type.clone()
    };
    let body = crate::protocol_proxy::protected_local::read_upstream_body_with_timeout(
        upstream.response,
        crate::protocol_proxy::protected_local::upstream_body_timeout(),
    )
    .await?;
    write_http_response(stream, &status, &content_type, &body).await?;
    log_helper_response(
        if is_success {
            "helper.audio_transcriptions_proxy_ok"
        } else {
            "helper.audio_transcriptions_proxy_upstream_error"
        },
        method,
        path,
        &status,
        remote_addr_text,
    );
    stream.shutdown().await?;
    Ok(())
}

pub(super) async fn handle_image_proxy_connection(
    stream: &mut tokio::net::TcpStream,
    request_body: &[u8],
    request_content_type: Option<&str>,
    request_user_agent: Option<&str>,
    method: &str,
    path: &str,
    remote_addr_text: Option<String>,
) -> anyhow::Result<()> {
    let upstream = if crate::protocol_proxy::protected_local::is_image_generations_proxy_path(path)
    {
        crate::protocol_proxy::protected_local::open_image_generations_proxy_request(
            request_body,
            request_user_agent,
        )
        .await
    } else {
        crate::protocol_proxy::protected_local::open_image_edits_proxy_request(
            request_body,
            request_content_type.unwrap_or_default(),
            request_user_agent,
        )
        .await
    };
    let upstream = match upstream {
        Ok(upstream) => upstream,
        Err(error) => {
            let body = serde_json::to_vec(&serde_json::json!({
                "status": "failed",
                "message": error.to_string()
            }))?;
            write_http_response(
                stream,
                "502 Bad Gateway",
                "application/json; charset=utf-8",
                &body,
            )
            .await?;
            log_helper_proxy_failure(
                "helper.image_proxy_failed",
                method,
                path,
                &error.to_string(),
                remote_addr_text,
            );
            stream.shutdown().await?;
            return Ok(());
        }
    };
    let status = upstream.response.status().to_string();
    let is_success = upstream.response.status().is_success();
    let content_type = if upstream.content_type.is_empty() {
        "application/json; charset=utf-8".to_string()
    } else {
        upstream.content_type.clone()
    };
    let body = upstream.response.bytes().await?.to_vec();
    write_http_response(stream, &status, &content_type, &body).await?;
    log_helper_response(
        if is_success {
            "helper.image_proxy_ok"
        } else {
            "helper.image_proxy_upstream_error"
        },
        method,
        path,
        &status,
        remote_addr_text,
    );
    stream.shutdown().await?;
    Ok(())
}

pub(super) async fn handle_chat_completions_proxy_connection(
    stream: &mut tokio::net::TcpStream,
    request_body: &str,
    request_user_agent: Option<&str>,
    method: &str,
    path: &str,
    remote_addr_text: Option<String>,
) -> anyhow::Result<()> {
    let upstream =
        match crate::protocol_proxy::protected_local::open_chat_completions_proxy_request(
            request_body,
            request_user_agent,
        )
        .await
        {
            Ok(upstream) => upstream,
            Err(error) => {
                let body = serde_json::to_vec(
                    &serde_json::json!({                 "status": "failed",                 "message": error.to_string()             }),
                )?;
                write_http_response(
                    stream,
                    "502 Bad Gateway",
                    "application/json; charset=utf-8",
                    &body,
                )
                .await?;
                log_helper_proxy_failure(
                    "helper.chat_completions_proxy_failed",
                    method,
                    path,
                    &error.to_string(),
                    remote_addr_text,
                );
                stream.shutdown().await?;
                return Ok(());
            }
        };
    let status = upstream.status();
    let is_success = upstream.is_success();
    let content_type = if upstream.content_type.is_empty() {
        "application/json; charset=utf-8".to_string()
    } else {
        upstream.content_type.clone()
    };
    if upstream.is_stream && is_success {
        write_http_stream_headers(stream, &status, &content_type).await?;
        let mut bytes_stream = Box::pin(upstream.response.bytes_stream());
        let mut stream_failed = false;
        loop {
            match crate::protocol_proxy::protected_local::next_stream_chunk_with_timeout(
                bytes_stream.as_mut(),
                crate::protocol_proxy::protected_local::upstream_stream_idle_timeout(),
            )
            .await
            {
                Ok(Some(Ok(chunk))) => stream.write_all(&chunk).await?,
                Ok(Some(Err(error))) => {
                    let _ = crate::diagnostic_log::append_diagnostic_log(
                        "helper.chat_completions_proxy_stream_failed",
                        serde_json::json!({
                            "method": method,
                            "path": path,
                            "reason": "stream_error",
                            "message": error.to_string(),
                            "remote_addr": remote_addr_text.clone(),
                        }),
                    );
                    stream_failed = true;
                    break;
                }
                Ok(None) => break,
                Err(crate::protocol_proxy::protected_local::StreamChunkWaitError::IdleTimeout) => {
                    let idle_timeout =
                        crate::protocol_proxy::protected_local::upstream_stream_idle_timeout()
                            .as_secs();
                    let _ = crate::diagnostic_log::append_diagnostic_log(
                        "helper.chat_completions_proxy_stream_idle_timeout",
                        serde_json::json!({
                            "method": method,
                            "path": path,
                            "idle_timeout_seconds": idle_timeout,
                            "remote_addr": remote_addr_text.clone(),
                        }),
                    );
                    stream_failed = true;
                    break;
                }
            }
        }
        log_helper_response(
            if stream_failed {
                "helper.chat_completions_proxy_stream_failed"
            } else {
                "helper.chat_completions_proxy_stream_ok"
            },
            method,
            path,
            &status,
            remote_addr_text,
        );
        stream.shutdown().await?;
        return Ok(());
    }
    let body = crate::protocol_proxy::protected_local::read_upstream_body_with_timeout(
        upstream.response,
        crate::protocol_proxy::protected_local::upstream_body_timeout(),
    )
    .await?;
    write_http_response(stream, &status, &content_type, &body).await?;
    log_helper_response(
        if is_success {
            "helper.chat_completions_proxy_ok"
        } else {
            "helper.chat_completions_proxy_upstream_error"
        },
        method,
        path,
        &status,
        remote_addr_text,
    );
    stream.shutdown().await?;
    Ok(())
}
