use super::parser::ParsedOutput;
use crate::model::{AgentId, EventKind};
use serde_json::Value;
use std::time::Duration;
use tokio::{sync::watch, time::Instant};

pub const LIMIT: Duration = Duration::from_secs(300);

// Thinking tokens and protocol keepalives must not postpone the watchdog.
pub fn is_progress(agent: AgentId, output: &ParsedOutput) -> bool {
    if output.kind == EventKind::Stderr {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(&output.raw) else {
        return false;
    };
    match agent {
        AgentId::Codex => {
            let method = value["method"].as_str().unwrap_or("");
            if method.contains("reasoning") {
                return false;
            }
            let item = value.get("item").unwrap_or(&value["params"]["item"]);
            if item["type"] == "reasoning" {
                return false;
            }
            output.summary.as_ref().is_some_and(|text| !text.is_empty())
                || matches!(
                    method,
                    "item/agentMessage/delta" | "item/mcpToolCall/outputDelta"
                )
        }
        AgentId::Claude => {
            if value["type"] == "stream_event" {
                let event = &value["event"];
                return (event["delta"]["type"] == "text_delta"
                    && event["delta"]["text"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty()))
                    || event["delta"]["type"] == "input_json_delta"
                    || event["content_block"]["type"] == "tool_use";
            }
            if value["type"] == "assistant" {
                return value["message"]["content"]
                    .as_array()
                    .is_some_and(|blocks| {
                        blocks
                            .iter()
                            .any(|b| b["type"] == "text" || b["type"] == "tool_use")
                    });
            }
            value["type"] == "result"
                || (value["type"] == "user"
                    && value["message"]["content"]
                        .as_array()
                        .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_result")))
        }
    }
}

pub struct Watchdog {
    sender: watch::Sender<Option<Instant>>,
    limit: Duration,
    remaining: std::sync::Mutex<Option<Duration>>,
}
impl Watchdog {
    pub fn new(limit: Duration) -> Self {
        let (sender, _) = watch::channel(Some(Instant::now() + limit));
        Self {
            sender,
            limit,
            remaining: std::sync::Mutex::new(None),
        }
    }
    pub fn progress(&self) {
        self.sender.send_replace(Some(Instant::now() + self.limit));
    }
    pub fn pause(&self) {
        self.sender.send_replace(None);
    }
    pub fn pause_budget(&self) {
        if let Some(deadline) = self.sender.send_replace(None) {
            *self.remaining.lock().unwrap() =
                Some(deadline.saturating_duration_since(Instant::now()));
        }
    }
    pub fn resume_budget(&self) {
        if let Some(remaining) = self.remaining.lock().unwrap().take() {
            self.sender.send_replace(Some(Instant::now() + remaining));
        }
    }
    pub async fn expired(&self) {
        let mut receiver = self.sender.subscribe();
        loop {
            let deadline = *receiver.borrow_and_update();
            if let Some(deadline) = deadline {
                tokio::select! { biased;
                    _ = receiver.changed() => {},
                    _ = tokio::time::sleep_until(deadline) => return,
                }
            } else {
                let _ = receiver.changed().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::parser::parse;
    use super::*;
    #[test]
    fn thinking_and_noise_do_not_reset_but_replies_and_tools_do() {
        for (agent, raw, expected) in [
            (
                AgentId::Codex,
                r#"{"method":"item/reasoning/textDelta","params":{"delta":"thinking"}}"#,
                false,
            ),
            (
                AgentId::Codex,
                r#"{"method":"item/started","params":{"item":{"type":"reasoning"}}}"#,
                false,
            ),
            (
                AgentId::Codex,
                r#"{"method":"item/agentMessage/delta","params":{"delta":"reply"}}"#,
                true,
            ),
            (
                AgentId::Codex,
                r#"{"method":"item/completed","params":{"item":{"type":"commandExecution"}}}"#,
                true,
            ),
            (
                AgentId::Claude,
                r#"{"type":"stream_event","event":{"delta":{"type":"thinking_delta","thinking":"thinking"}}}"#,
                false,
            ),
            (
                AgentId::Claude,
                r#"{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"reply"}}}"#,
                true,
            ),
            (
                AgentId::Claude,
                r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"thinking"}]}}"#,
                false,
            ),
            (
                AgentId::Claude,
                r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#,
                true,
            ),
            (AgentId::Claude, r#"{"type":"ping"}"#, false),
        ] {
            assert_eq!(
                is_progress(agent, &parse(agent, raw, false)),
                expected,
                "{raw}"
            );
        }
    }
    #[tokio::test]
    async fn pauses_for_user_and_restarts_after_delivery() {
        let timer = Watchdog::new(Duration::from_millis(40));
        timer.pause();
        assert!(
            tokio::time::timeout(Duration::from_millis(80), timer.expired())
                .await
                .is_err()
        );
        timer.progress();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), timer.expired())
                .await
                .is_err()
        );
        timer.progress();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), timer.expired())
                .await
                .is_err()
        );
        tokio::time::timeout(Duration::from_millis(150), timer.expired())
            .await
            .unwrap();
        assert_eq!(LIMIT.as_secs(), 300);
    }
}
