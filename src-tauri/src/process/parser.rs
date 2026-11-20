use crate::model::{AgentId, EventKind};
use serde::Serialize;
use serde_json::Value;

// Summaries contain selected human-readable fields, never serialized protocol objects.
fn tool_summary(item: &Value, started: bool) -> String {
    let kind = item["type"].as_str().unwrap_or("");
    let name = match kind {
        "commandExecution" | "command_execution" => "执行命令",
        "fileChange" | "file_change" => "修改文件",
        "mcpToolCall" | "mcp_tool_call" => "调用工具",
        "webSearch" | "web_search" => "搜索网页",
        "reasoning" => "思考",
        "plan" => "制定计划",
        _ => "工具活动",
    };
    let mut fields = vec![format!(
        "{} · {name}",
        if started { "开始" } else { "完成" }
    )];
    for key in ["server", "tool", "name", "command", "query", "status"] {
        if let Some(text) = item[key].as_str().filter(|s| !s.is_empty()) {
            fields.push(text.to_owned());
        }
    }
    if let Some(code) = item["exitCode"].as_i64().or(item["exit_code"].as_i64()) {
        fields.push(format!("退出码 {code}"));
    }
    if let Some(changes) = item["changes"].as_array() {
        fields.extend(
            changes
                .iter()
                .filter_map(|c| c["path"].as_str())
                .map(str::to_owned),
        );
    }
    fields.join(" · ")
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedOutput {
    pub interaction: Option<crate::model::Interaction>,
    pub resolved_interaction: Option<String>,
    pub kind: EventKind,
    pub raw: String,
    pub session_id: Option<String>,
    pub summary: Option<String>,
    pub reported_failure: bool,
}

pub fn parse(agent: AgentId, line: &str, stderr: bool) -> ParsedOutput {
    let mut parsed = ParsedOutput {
        interaction: None,
        resolved_interaction: None,
        kind: if stderr {
            EventKind::Stderr
        } else {
            EventKind::Stdout
        },
        raw: line.into(),
        session_id: None,
        summary: None,
        reported_failure: false,
    };
    if stderr {
        return parsed;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return parsed;
    };
    let event_type = value["type"].as_str().unwrap_or("");
    match agent {
        AgentId::Codex => {
            let method = value["method"].as_str().unwrap_or("");
            if matches!(method, "item/started" | "item/completed") {
                let item = &value["params"]["item"];
                if item["type"] != "agentMessage" && item["type"] != "userMessage" {
                    parsed.kind = EventKind::ToolSummary;
                    parsed.summary = Some(tool_summary(item, method == "item/started"));
                }
            }
            if matches!(
                method,
                "item/agentMessage/delta"
                    | "item/reasoning/textDelta"
                    | "item/reasoning/summaryTextDelta"
                    | "item/commandExecution/outputDelta"
                    | "item/fileChange/outputDelta"
            ) {
                parsed.summary = value["params"]["delta"].as_str().map(str::to_owned);
                parsed.kind = if method == "item/agentMessage/delta" {
                    EventKind::Message
                } else {
                    EventKind::ToolSummary
                };
            }
            if event_type == "thread.started" {
                parsed.session_id = value["thread_id"].as_str().map(str::to_owned);
            }
            if matches!(
                event_type,
                "item.started" | "item.updated" | "item.completed"
            ) {
                let item = &value["item"];
                match item["type"].as_str() {
                    Some("agent_message") => {
                        parsed.kind = EventKind::Message;
                        parsed.summary = item["text"].as_str().map(str::to_owned);
                    }
                    Some("command_execution" | "mcp_tool_call" | "file_change" | "web_search") => {
                        parsed.kind = EventKind::ToolSummary;
                        parsed.summary = Some(tool_summary(item, event_type == "item.started"));
                    }
                    _ => {}
                }
            }
            parsed.reported_failure = matches!(event_type, "turn.failed" | "error");
        }
        AgentId::Claude => {
            parsed.session_id = value["session_id"].as_str().map(str::to_owned);
            if event_type == "stream_event" {
                let event = &value["event"];
                if event["type"] == "content_block_delta" {
                    let delta = &event["delta"];
                    if delta["type"] == "text_delta" {
                        parsed.kind = EventKind::Message;
                        parsed.summary = delta["text"].as_str().map(str::to_owned);
                    } else if delta["type"] == "thinking_delta" {
                        parsed.kind = EventKind::ToolSummary;
                        parsed.summary = delta["thinking"].as_str().map(str::to_owned);
                    }
                } else if event["type"] == "content_block_start"
                    && event["content_block"]["type"] == "tool_use"
                {
                    parsed.kind = EventKind::ToolSummary;
                    parsed.summary = event["content_block"]["name"]
                        .as_str()
                        .map(|name| format!("开始 · {name}"));
                }
            } else if event_type == "assistant" {
                if let Some(blocks) = value["message"]["content"].as_array() {
                    let tool = blocks.iter().any(|b| b["type"] == "tool_use");
                    parsed.kind = if tool {
                        EventKind::ToolSummary
                    } else {
                        EventKind::Message
                    };
                    let summaries: Vec<_> = blocks
                        .iter()
                        .filter_map(|b| b["text"].as_str().or(b["name"].as_str()))
                        .collect();
                    if !summaries.is_empty() {
                        parsed.summary = Some(summaries.join("\n"));
                    }
                }
            } else if event_type == "result" {
                parsed.kind = EventKind::Message;
                parsed.summary = value["result"].as_str().map(str::to_owned);
                parsed.reported_failure = value["is_error"].as_bool().unwrap_or(false);
            }
        }
    }
    parsed
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_incremental_text_without_exposing_tool_json() {
        let text = parse(
            AgentId::Claude,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"你好 world"}}}"#,
            false,
        );
        assert_eq!(text.kind, EventKind::Message);
        assert_eq!(text.summary.as_deref(), Some("你好 world"));
        let input = parse(
            AgentId::Claude,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}}"#,
            false,
        );
        assert!(input.summary.is_none());
        let tool = parse(
            AgentId::Codex,
            r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","command":"echo hello","exitCode":0,"aggregatedOutput":"large output","internal":{"secret":"hidden"}}}}"#,
            false,
        );
        assert_eq!(
            tool.summary.as_deref(),
            Some("完成 · 执行命令 · echo hello · 退出码 0")
        );
        assert!(tool.raw.contains("hidden"));
        let delta = parse(
            AgentId::Codex,
            r#"{"method":"item/commandExecution/outputDelta","params":{"delta":"hello\n"}}"#,
            false,
        );
        assert_eq!(delta.kind, EventKind::ToolSummary);
        assert_eq!(delta.summary.as_deref(), Some("hello\n"));
    }
    #[test]
    fn extracts_codex_session_and_tools() {
        assert_eq!(
            parse(
                AgentId::Codex,
                r#"{"type":"thread.started","thread_id":"abc"}"#,
                false
            )
            .session_id
            .as_deref(),
            Some("abc")
        );
        assert_eq!(
            parse(
                AgentId::Codex,
                r#"{"type":"item.completed","item":{"type":"mcp_tool_call","tool":"inspect"}}"#,
                false
            )
            .kind,
            EventKind::ToolSummary
        );
    }
    #[test]
    fn retains_original_agent_language_and_unknown_json() {
        let line = r#"{"type":"assistant","session_id":"xyz","message":{"content":[{"type":"text","text":"已完成"}]}}"#;
        let parsed = parse(AgentId::Claude, line, false);
        assert_eq!(parsed.raw, line);
        assert_eq!(parsed.summary.as_deref(), Some("已完成"));
        assert_eq!(
            parse(AgentId::Codex, "{\"future\":true}", false).kind,
            EventKind::Stdout
        );
        assert_eq!(parse(AgentId::Claude, "not json", false).raw, "not json");
    }
    #[test]
    fn flags_result_errors_without_inferring_state_transitions() {
        assert!(
            parse(
                AgentId::Claude,
                r#"{"type":"result","is_error":true}"#,
                false
            )
            .reported_failure
        );
        assert!(parse(AgentId::Codex, r#"{"type":"turn.failed"}"#, false).reported_failure);
        assert_eq!(
            parse(AgentId::Claude, "stderr", true).kind,
            EventKind::Stderr
        );
    }
}
