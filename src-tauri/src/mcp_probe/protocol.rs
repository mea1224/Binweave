use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncWrite, AsyncWriteExt};

// REA tool schemas can exceed 1 MiB in a single tools/list response.
// Keep an explicit allocation bound while allowing realistic catalogs.
const MAX_FRAME: u64 = 16 * 1024 * 1024;
async fn send<W: AsyncWrite + Unpin>(writer: &mut W, message: Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(&message).map_err(|_| "Unable to encode MCP request")?;
    writer
        .write_all(&bytes)
        .await
        .map_err(|_| "MCP stdin closed")?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|_| "MCP stdin closed")?;
    writer
        .flush()
        .await
        .map_err(|_| "MCP stdin flush failed".into())
}
async fn response<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    id: u64,
) -> Result<Value, String> {
    let message = envelope(reader, writer, id).await?;
    if let Some(error) = message.get("error") {
        return Err(format!(
            "MCP request failed with JSON-RPC code {}",
            error["code"].as_i64().unwrap_or(-32603)
        ));
    }
    Ok(message["result"].clone())
}

async fn envelope<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    id: u64,
) -> Result<Value, String> {
    loop {
        let line = crate::process::framing::next_line(reader, MAX_FRAME as usize)
            .await?
            .ok_or("MCP server closed stdout before responding")?;
        let message: Value =
            serde_json::from_str(&line).map_err(|_| "MCP stdout contains invalid JSON")?;
        if message["jsonrpc"] != "2.0" {
            return Err("MCP message is not JSON-RPC 2.0".into());
        }
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            if let Some(request_id) = message.get("id") {
                let reply = if method == "ping" {
                    json!({"jsonrpc":"2.0","id":request_id,"result":{}})
                } else {
                    json!({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"Method not supported by diagnostic client"}})
                };
                send(writer, reply).await?;
            }
            continue;
        }
        if message.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if message.get("result").is_some() == message.get("error").is_some()
            || message
                .get("error")
                .is_some_and(|e| !e["code"].is_i64() || !e["message"].is_string())
        {
            return Err("Invalid MCP response envelope".into());
        }
        return Ok(message);
    }
}
pub async fn handshake<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
) -> Result<Vec<Value>, String> {
    send(writer, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"binweave","version":"0.1.0"}}})).await?;
    let initialized = response(reader, writer, 1).await?;
    if !matches!(
        initialized["protocolVersion"].as_str(),
        Some("2025-06-18" | "2025-03-26" | "2024-11-05")
    ) {
        return Err("Unsupported negotiated MCP protocol version".into());
    }
    if !initialized["serverInfo"].is_object() || !initialized["capabilities"]["tools"].is_object() {
        return Err("Invalid server information or missing tools capability".into());
    }
    send(
        writer,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await?;
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for id in 2..18 {
        let params = cursor
            .as_ref()
            .map_or_else(|| json!({}), |cursor| json!({"cursor":cursor}));
        send(
            writer,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/list","params":params}),
        )
        .await?;
        let result = response(reader, writer, id).await?;
        let page = result["tools"]
            .as_array()
            .ok_or("MCP tools/list omitted the tools array")?;
        for tool in page {
            let name = tool["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("MCP tool has no name")?;
            if !tool["inputSchema"].is_object() {
                return Err("MCP tool has no input schema".into());
            }
            if tools
                .iter()
                .any(|existing: &Value| existing["name"] == name)
            {
                return Err("MCP 工具名称重复，无法可靠验证".into());
            }
            tools.push(tool.clone());
        }
        cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            return Ok(tools);
        }
    }
    Err("MCP tools/list exceeded the 16-page diagnostic limit".into())
}

// Discovery, parameter planning and invocation outcomes remain separate.
pub async fn verify_tools<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    tools: Vec<Value>,
) -> Vec<crate::model::McpToolCheck> {
    use std::time::Duration;
    use tokio::time::{timeout_at, Instant};
    let deadline = Instant::now() + Duration::from_secs(20);
    use super::strategy::{self, Context, Plan};
    let mut context = Context::default();
    let mut checks: Vec<_> = tools
        .iter()
        .map(|tool| crate::model::McpToolCheck {
            name: tool["name"].as_str().unwrap_or_default().into(),
            status: "not_checked".into(),
            detail: String::new(),
            outcome: "needs_context".into(),
            method: "none".into(),
        })
        .collect();
    let mut pending: Vec<usize> = (0..tools.len()).collect();
    pending.sort_by_key(|i| strategy::priority(checks[*i].name.as_str()));
    let mut channel_open = true;
    // Revisit only tools that have not been invoked and still need context.
    // Every invocation happens at most once, even after a recoverable error.
    loop {
        let mut deferred = Vec::new();
        let mut progressed = false;
        for index in pending {
            let check = &mut checks[index];
            let arguments = match strategy::plan(&tools[index], &context) {
                Plan::Pending(outcome, detail) => {
                    check.outcome = outcome.into();
                    check.detail = detail.into();
                    if outcome == "needs_context" {
                        deferred.push(index);
                    }
                    continue;
                }
                Plan::Call(arguments) => arguments,
            };
            if !channel_open || Instant::now() >= deadline {
                check.outcome = "interrupted".into();
                check.detail = if channel_open {
                    "服务验证预算已用完，未调用此工具。"
                } else {
                    "连接故障或超时后通道已停止，未调用此工具。"
                }
                .into();
                continue;
            }
            progressed = true;
            check.method = if arguments.as_object().is_some_and(|a| a.is_empty()) {
                "empty"
            } else {
                "context"
            }
            .into();
            let id = 100 + index as u64;
            let call = async {
                send(writer, json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":check.name,"arguments":arguments}})).await?;
                envelope(reader, writer, id).await
            };
            match timeout_at(deadline.min(Instant::now() + Duration::from_secs(3)), call).await {
                Ok(Ok(message)) => {
                    if let Some(error) = message.get("error") {
                        let (outcome, detail) = if error["code"] == -32601 {
                            ("unavailable", "服务端明确不支持此工具调用。")
                        } else {
                            strategy::failure(error)
                        };
                        check.outcome = outcome.into();
                        check.status = if outcome == "needs_context" {
                            "not_checked"
                        } else {
                            "error"
                        }
                        .into();
                        check.detail = format!("{detail} JSON-RPC code {}。", error["code"]);
                        continue;
                    }
                    let result = &message["result"];
                    let content = result.get("content").and_then(Value::as_array);
                    let valid = content.is_some_and(|items| {
                        items.iter().all(|item| match item["type"].as_str() {
                            Some("text") => item["text"].is_string(),
                            Some("image" | "audio") => {
                                item["data"].is_string() && item["mimeType"].is_string()
                            }
                            Some("resource") => item["resource"]["uri"].is_string(),
                            Some("resource_link") => {
                                item["uri"].is_string() && item["name"].is_string()
                            }
                            _ => false,
                        })
                    });
                    if !valid || result.get("isError").is_some_and(|v| !v.is_boolean()) {
                        check.status = "error".into();
                        check.outcome = "failed".into();
                        check.detail = "工具返回无效结果，未验证可用。".into();
                    } else if result["isError"] == true {
                        let (outcome, detail) = strategy::failure(result);
                        check.outcome = outcome.into();
                        check.status = if outcome == "needs_context" {
                            "not_checked"
                        } else {
                            "error"
                        }
                        .into();
                        check.detail = detail.into();
                    } else {
                        check.status = "ok".into();
                        check.outcome = "verified".into();
                        check.detail = "查询 tools/call 成功；仅验证当前环境与此次参数。".into();
                        context.observe(&check.name, result);
                    }
                }
                Ok(Err(_)) => {
                    check.status = "error".into();
                    check.outcome = "failed".into();
                    check.detail = "工具调用协议失败；未保存服务端错误正文。".into();
                    channel_open = false;
                }
                Err(_) => {
                    check.status = "error".into();
                    check.outcome = "timeout".into();
                    check.detail = "工具调用验证超时；为避免复用不完整帧，停止当前通道。".into();
                    channel_open = false;
                }
            }
        }
        if !progressed || deferred.is_empty() {
            break;
        }
        pending = deferred;
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, split, AsyncBufReadExt, BufReader};
    #[tokio::test]
    async fn only_successful_calls_are_verified_and_unsafe_tools_are_not_called() {
        let tools = vec![
            json!({"name":"declared_only","inputSchema":{"type":"object"}}),
            json!({"name":"needs_file","annotations":{"readOnlyHint":true},"inputSchema":{"type":"object","required":["path"]}}),
            json!({"name":"complex","annotations":{"readOnlyHint":true},"inputSchema":{"type":"object","minProperties":1}}),
            json!({"name":"ready","annotations":{"readOnlyHint":true},"inputSchema":{"type":"object"}}),
            json!({"name":"fails","annotations":{"readOnlyHint":true},"inputSchema":{"type":"object"}}),
            json!({"name":"invalid","annotations":{"readOnlyHint":true},"inputSchema":{"type":"object"}}),
        ];
        let (client, server) = duplex(8192);
        let (r, mut w) = split(client);
        let task = tokio::spawn(async move {
            let (r, mut w) = split(server);
            let mut lines = BufReader::new(r).lines();
            for (name, result) in [
                ("ready", json!({"content":[{"type":"text","text":"ok"}]})),
                (
                    "fails",
                    json!({"isError":true,"content":[{"type":"text","text":"secret failure"}]}),
                ),
                ("invalid", json!({"content":[{"type":"text"}]})),
            ] {
                let request: Value =
                    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert_eq!(request["method"], "tools/call");
                assert_eq!(request["params"]["name"], name);
                assert_eq!(request["params"]["arguments"], json!({}));
                send(
                    &mut w,
                    json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
                )
                .await
                .unwrap();
            }
        });
        let checks = verify_tools(&mut BufReader::new(r), &mut w, tools).await;
        assert_eq!(
            checks.iter().map(|c| c.status.as_str()).collect::<Vec<_>>(),
            vec![
                "not_checked",
                "not_checked",
                "not_checked",
                "ok",
                "error",
                "error"
            ]
        );
        assert!(!serde_json::to_string(&checks).unwrap().contains("secret"));
        task.await.unwrap();
    }
    #[tokio::test]
    async fn rpc_error_is_local_to_tool_while_broken_frame_stops_channel() {
        let tool = json!({"name":"read","annotations":{"readOnlyHint":true},"inputSchema":{"type":"object"}});
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":100,\"error\":{\"code\":-32601,\"message\":\"secret\"}}\n{\"jsonrpc\":\"2.0\",\"id\":101,\"result\":{\"content\":[]}}\ninvalid\n";
        let checks = verify_tools(
            &mut &input[..],
            &mut tokio::io::sink(),
            vec![tool.clone(), tool.clone(), tool.clone(), tool],
        )
        .await;
        assert_eq!(checks[0].status, "error");
        assert_eq!(checks[0].outcome, "unavailable");
        assert_eq!(checks[1].status, "ok");
        assert_eq!(checks[2].outcome, "failed");
        assert_eq!(checks[3].outcome, "interrupted");
        assert!(!serde_json::to_string(&checks).unwrap().contains("secret"));
    }
    #[tokio::test]
    async fn observes_context_before_parameterized_calls_and_isolates_read_bytes_failure() {
        let tools = vec![
            json!({"name":"procedure_pseudo_code","inputSchema":{"type":"object","required":["procedure"],"properties":{"procedure":{"type":"string"},"document":{"type":"string"}}}}),
            json!({"name":"read_bytes","inputSchema":{"type":"object","required":["address"],"properties":{"address":{"type":"string"},"length":{"type":"integer","minimum":1}}}}),
            json!({"name":"list_documents","inputSchema":{"type":"object"}}),
            json!({"name":"list_procedures","inputSchema":{"type":"object","required":["document"],"properties":{"document":{"type":"string"}}}}),
            json!({"name":"xrefs","inputSchema":{"type":"object","required":["address"],"properties":{"address":{"type":"string"}}}}),
            json!({"name":"set_comment","annotations":{"readOnlyHint":true},"inputSchema":{"type":"object"}}),
        ];
        let (client, server) = duplex(8192);
        let (r, mut w) = split(client);
        let task = tokio::spawn(async move {
            let (r, mut w) = split(server);
            let mut lines = BufReader::new(r).lines();
            for (name, args, result) in [
                (
                    "list_documents",
                    json!({}),
                    json!({"content":[],"structuredContent":{"raw_result":["fixture.exe"]}}),
                ),
                (
                    "list_procedures",
                    json!({"document":"fixture.exe"}),
                    json!({"content":[{"type":"text","text":"{\"raw_result\":[{\"name\":\"entry\",\"address\":\"0x1234\"}]}"}]}),
                ),
                (
                    "procedure_pseudo_code",
                    json!({"document":"fixture.exe","procedure":"entry"}),
                    json!({"content":[{"type":"text","text":"int entry() {}"}]}),
                ),
                (
                    "read_bytes",
                    json!({"address":"0x1234","length":1}),
                    json!({"isError":true,"content":[{"type":"text","text":"capability_unavailable secret-token"}]}),
                ),
                ("xrefs", json!({"address":"0x1234"}), json!({"content":[]})),
            ] {
                let request: Value =
                    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert_eq!(request["params"], json!({"name":name,"arguments":args}));
                send(
                    &mut w,
                    json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
                )
                .await
                .unwrap();
            }
        });
        let checks = verify_tools(&mut BufReader::new(r), &mut w, tools).await;
        assert_eq!(checks[0].outcome, "verified");
        assert_eq!(checks[0].method, "context");
        assert_eq!(checks[1].outcome, "unavailable");
        assert_eq!(checks[4].outcome, "verified");
        assert_eq!(checks[5].outcome, "approval_required");
        assert!(!serde_json::to_string(&checks)
            .unwrap()
            .contains("secret-token"));
        task.await.unwrap();
    }
    #[tokio::test]
    async fn timeout_stops_partial_frame_reuse_without_marking_remaining_tools_failed() {
        let (client, server) = duplex(8192);
        let (r, mut w) = split(client);
        let task = tokio::spawn(async move {
            let (r, mut w) = split(server);
            let mut lines = BufReader::new(r).lines();
            lines.next_line().await.unwrap();
            w.write_all(b"{\"jsonrpc\":").await.unwrap();
            std::future::pending::<()>().await;
        });
        let tool = json!({"name":"list_documents","inputSchema":{"type":"object"}});
        let checks = verify_tools(&mut BufReader::new(r), &mut w, vec![tool.clone(), tool]).await;
        assert_eq!(checks[0].outcome, "timeout");
        assert_eq!(checks[1].outcome, "interrupted");
        task.abort();
        let _ = task.await;
    }
    #[tokio::test]
    async fn completes_ordered_handshake_and_pagination() {
        let (client, server) = duplex(8192);
        let (read, mut write) = split(client);
        let task = tokio::spawn(async move {
            let (r, mut w) = split(server);
            let mut lines = BufReader::new(r).lines();
            let init: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(init["method"], "initialize");
            send(&mut w, json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","serverInfo":{"name":"fixture","version":"1"},"capabilities":{"tools":{}}}})).await.unwrap();
            let notify: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(notify["method"], "notifications/initialized");
            for id in 2..4 {
                let request: Value =
                    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert_eq!(request["method"], "tools/list");
                if id == 3 {
                    assert_eq!(request["params"]["cursor"], "page-2");
                }
                let result = if id == 2 {
                    json!({"tools":[],"nextCursor":"page-2"})
                } else {
                    json!({"tools":[{"name":"open_binary","inputSchema":{"type":"object"}}]})
                };
                send(&mut w, json!({"jsonrpc":"2.0","id":id,"result":result}))
                    .await
                    .unwrap();
            }
        });
        let tools = handshake(&mut BufReader::new(read), &mut write)
            .await
            .unwrap();
        assert_eq!(tools[0]["name"], "open_binary");
        task.await.unwrap();
    }
    #[tokio::test]
    async fn rejects_rpc_errors_without_leaking_text() {
        let bytes = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32600,\"message\":\"secret-token\"}}\n";
        let error = handshake(&mut &bytes[..], &mut tokio::io::sink())
            .await
            .unwrap_err();
        assert!(error.contains("-32600"));
        assert!(!error.contains("secret"));
    }
    #[tokio::test]
    async fn rejects_eof_and_invalid_json() {
        assert!(handshake(&mut &b""[..], &mut tokio::io::sink())
            .await
            .is_err());
        assert!(handshake(&mut &b"not-json\n"[..], &mut tokio::io::sink())
            .await
            .is_err());
    }
    #[tokio::test]
    async fn stalled_server_is_bounded_by_deadline() {
        let (client, _server) = duplex(4096);
        let (r, mut w) = split(client);
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(30),
            handshake(&mut BufReader::new(r), &mut w),
        )
        .await;
        assert!(result.is_err());
    }
    #[tokio::test]
    async fn accepts_large_rea_tool_catalog_with_bounded_frames() {
        let init = json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","serverInfo":{"name":"fixture","version":"1"},"capabilities":{"tools":{}}}});
        let tools = json!({"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"large_tool","description":"x".repeat(2 * 1024 * 1024),"inputSchema":{"type":"object"}}]}});
        let input = format!("{init}\n{tools}\n");
        let names = handshake(&mut input.as_bytes(), &mut tokio::io::sink())
            .await
            .unwrap();
        assert_eq!(names[0]["name"], "large_tool");
        let huge = vec![b'x'; MAX_FRAME as usize + 1];
        assert!(handshake(&mut &huge[..], &mut tokio::io::sink())
            .await
            .is_err());
    }
}
