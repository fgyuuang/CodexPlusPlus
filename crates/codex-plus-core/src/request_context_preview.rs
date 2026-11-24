use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

pub(crate) const PATH: &str = "/internal/request-context-preview";
const TTL: Duration = Duration::from_secs(600);
const MAX_REQUESTS: usize = 10;
const MAX_MESSAGES: usize = 8;
const MAX_CHARS: usize = 320;
static TOKEN: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
});
static CAPTURE: Mutex<Option<Capture>> = Mutex::new(None);

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRequest {
    action: Action,
    agent: String,
    session: String,
    request_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum Action {
    Enable,
    Stop,
    Read,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snippet {
    role: Role,
    text: String,
    truncated: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum Role {
    User,
    Assistant,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct PreviewResponse {
    active: bool,
    snippets: Vec<Snippet>,
}

struct Capture {
    agent: String,
    session: String,
    deadline: Instant,
    requests: VecDeque<(String, Vec<Snippet>)>,
}

impl Capture {
    fn matches(&self, agent: &str, session: &str) -> bool {
        self.deadline > Instant::now() && self.agent == agent && self.session == session
    }

    fn put(&mut self, id: &str, snippets: Vec<Snippet>) {
        self.requests.retain(|(old_id, _)| old_id != id);
        if self.requests.len() >= MAX_REQUESTS {
            self.requests.pop_front();
        }
        self.requests.push_back((id.to_owned(), snippets));
    }
}

pub(crate) fn token() -> &'static str {
    &TOKEN
}

pub(crate) fn record(agent: &str, session: &str, id: &str, body: &Value) {
    let Ok(mut slot) = CAPTURE.lock() else { return };
    if let Some(capture) = slot
        .as_mut()
        .filter(|capture| capture.matches(agent, session))
    {
        let snippets = message_snippets(body);
        if !snippets.is_empty() {
            capture.put(id, snippets);
        }
    }
}

fn message_snippets(body: &Value) -> Vec<Snippet> {
    let mut found = Vec::new();
    let mut add = |role, text: &str| {
        let mut chars = text.chars();
        let text: String = chars.by_ref().take(MAX_CHARS).collect();
        if !text.trim().is_empty() && found.len() < MAX_MESSAGES {
            found.push(Snippet {
                role,
                text,
                truncated: chars.next().is_some(),
            });
        }
    };
    match body.get("input") {
        Some(Value::String(text)) => add(Role::User, text),
        Some(Value::Array(items)) => {
            for item in items.iter().rev().take(64) {
                if !matches!(
                    item.get("type").and_then(Value::as_str),
                    None | Some("message")
                ) {
                    continue;
                }
                let role = match item.get("role").and_then(Value::as_str) {
                    Some("user") => Role::User,
                    Some("assistant") => Role::Assistant,
                    _ => continue,
                };
                match item.get("content") {
                    Some(Value::String(text)) => add(role, text),
                    Some(Value::Array(blocks)) => {
                        for block in blocks.iter().rev().take(16) {
                            if matches!(
                                block.get("type").and_then(Value::as_str),
                                Some("text" | "input_text" | "output_text")
                            ) && let Some(text) = block.get("text").and_then(Value::as_str)
                            {
                                add(role, text);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    found.reverse();
    found
}

fn authorized(method: &str, loopback: bool, headers: &str) -> bool {
    use crate::launcher::header_value_from_headers as header;
    method == "POST"
        && loopback
        && header(headers, "origin").is_none()
        && header(headers, "authorization")
            .as_deref()
            .and_then(|value| value.strip_prefix("Bearer "))
            == Some(token())
}

fn handle(request: PreviewRequest) -> PreviewResponse {
    let Ok(mut slot) = CAPTURE.lock() else {
        return PreviewResponse::default();
    };
    let PreviewRequest {
        action,
        agent,
        session,
        request_id,
    } = request;
    if agent.is_empty() || agent.len() > 64 || session.is_empty() || session.len() > 512 {
        return PreviewResponse::default();
    }
    if matches!(action, Action::Enable) {
        let deadline = Instant::now() + TTL;
        *slot = Some(Capture {
            agent,
            session,
            deadline,
            requests: VecDeque::new(),
        });
        tokio::spawn(async move {
            tokio::time::sleep_until(deadline.into()).await;
            if let Ok(mut slot) = CAPTURE.lock()
                && slot
                    .as_ref()
                    .is_some_and(|existing| existing.deadline == deadline)
            {
                *slot = None;
            }
        });
        return PreviewResponse {
            active: true,
            ..PreviewResponse::default()
        };
    }
    let Some(capture) = slot
        .as_ref()
        .filter(|capture| capture.matches(&agent, &session))
    else {
        return PreviewResponse::default();
    };
    if matches!(action, Action::Stop) {
        *slot = None;
        return PreviewResponse::default();
    }
    let snippets = request_id
        .as_deref()
        .and_then(|wanted| {
            capture
                .requests
                .iter()
                .find(|(id, _)| id == wanted)
                .map(|(_, snippets)| snippets.clone())
        })
        .unwrap_or_default();
    PreviewResponse {
        active: true,
        snippets,
    }
}

pub(crate) async fn serve(
    stream: &mut tokio::net::TcpStream,
    method: &str,
    headers: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    let loopback = stream.peer_addr().is_ok_and(|addr| addr.ip().is_loopback());
    let (status, payload) = if !authorized(method, loopback, headers) {
        ("403 Forbidden", Vec::new())
    } else if let Some(request) = (body.len() <= 4096)
        .then(|| serde_json::from_slice::<PreviewRequest>(body).ok())
        .flatten()
    {
        ("200 OK", serde_json::to_vec(&handle(request))?)
    } else {
        ("400 Bad Request", Vec::new())
    };
    // No CORS or cache headers from the generic helper response writer.
    stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len()).as_bytes()).await?;
    stream.write_all(&payload).await?;
    stream.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn excerpts_are_opt_in_session_scoped_bounded_and_discarded_on_stop() {
        let request = |action, agent: &str, session: &str, id: Option<&str>| PreviewRequest {
            action,
            agent: agent.into(),
            session: session.into(),
            request_id: id.map(str::to_owned),
        };
        let snippet = "你".repeat(321);
        let input = json!({"instructions": "private-instructions", "input": [
            {"role":"developer","content":"private-developer"},
            {"type":"function_call_output","output":"private-tool"},
            {"role":"assistant","content":"previous answer"},
            {"role":"user","content":[{"type":"input_image","image_url":"private-media"},{"type":"input_text","text":snippet}]}
        ]});
        record("codex", "session", "before", &input);
        assert!(handle(request(Action::Enable, "codex", "session", None)).active);
        record("claude", "session", "other-agent", &input);
        record("codex", "other", "other-session", &input);
        record("codex", "session", "evicted", &input);
        for index in 0..MAX_REQUESTS {
            record("codex", "session", &index.to_string(), &input);
        }
        for absent in ["before", "other-agent", "other-session", "evicted"] {
            assert!(
                handle(request(Action::Read, "codex", "session", Some(absent)))
                    .snippets
                    .is_empty()
            );
        }
        assert!(!handle(request(Action::Read, "codex", "other", Some("0"))).active);
        let result = handle(request(Action::Read, "codex", "session", Some("0")));
        assert_eq!(result.snippets.len(), 2);
        assert_eq!(result.snippets[0].text, "previous answer");
        assert_eq!(result.snippets[1].text.chars().count(), MAX_CHARS);
        assert!(result.snippets[1].truncated);
        assert!(!serde_json::to_string(&result).unwrap().contains("private-"));
        assert!(!handle(request(Action::Stop, "codex", "session", None)).active);
        assert!(
            handle(request(Action::Read, "codex", "session", Some("0")))
                .snippets
                .is_empty()
        );
    }

    #[test]
    fn preview_requires_its_own_capability_and_rejects_browser_and_remote_requests() {
        let line = "POST /internal/request-context-preview HTTP/1.1\r\n";
        let headers = format!("{line}Authorization: Bearer {}\r\n", token());
        assert!(authorized("POST", true, &headers));
        assert!(!authorized("POST", false, &headers));
        assert!(!authorized("GET", true, &headers));
        assert!(!authorized("POST", true, line));
        assert!(!authorized(
            "POST",
            true,
            &format!("{headers}Origin: null\r\n")
        ));
        assert!(!authorized(
            "POST",
            true,
            &format!(
                "{line}Authorization: Bearer {}\r\n",
                crate::dictation::helper_token()
            )
        ));
    }
}
