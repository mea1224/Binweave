//! Scoped, duplex CLI transports. Replies are never submitted as new prompts.
use super::{
    framing,
    parser::{self, ParsedOutput},
    stream::{RunError, RunSpec},
};
use crate::model::{AgentId, Interaction, InteractionQuestion};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    process::Stdio,
    sync::{Arc, Mutex},
};
use tokio::{
    io::AsyncWriteExt,
    sync::{mpsc, oneshot},
};

pub struct Config {
    pub elicitation_policy: crate::model::ElicitationPolicy,
    pub analyst: bool,
    pub broker: Arc<Broker>,
    pub run_id: String,
    pub prompt_id: String,
    pub session: Option<String>,
}
struct Reply {
    id: String,
    answer: Value,
    ack: oneshot::Sender<Result<(), String>>,
}
#[derive(Default)]
pub struct Broker {
    pending: Mutex<HashMap<String, (String, String, mpsc::Sender<Reply>)>>,
    // Deliberately memory-only: never grant permanent or cross-workspace permission.
    remembered: Mutex<HashSet<String>>,
}
impl Broker {
    pub async fn respond(
        &self,
        run: &str,
        turn: &str,
        id: &str,
        answer: Value,
    ) -> Result<(), String> {
        if answer.to_string().len() > 64 * 1024 {
            return Err("回答超过长度限制".into());
        }
        let sender = {
            let entries = self.pending.lock().map_err(|_| "交互通道锁异常")?;
            let (r, t, sender) = entries.get(id).ok_or("交互已过期或已经回答")?;
            if r != run || t != turn {
                return Err("交互不属于当前轮次".into());
            }
            sender.clone()
        };
        let (ack, result) = oneshot::channel();
        sender
            .send(Reply {
                id: id.into(),
                answer,
                ack,
            })
            .await
            .map_err(|_| "Agent 通道已关闭")?;
        result.await.map_err(|_| "Agent 已停止，响应未确认写入")?
    }
}
struct Cleanup<'a>(&'a Config);
impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        if let Ok(mut map) = self.0.broker.pending.lock() {
            map.retain(|_, (r, t, _)| r != &self.0.run_id || t != &self.0.prompt_id);
        }
    }
}
fn command_request(agent: AgentId, value: &Value) -> bool {
    let command = match agent {
        AgentId::Codex if value["method"] == "item/commandExecution/requestApproval" => {
            &value["params"]["command"]
        }
        AgentId::Claude
            if value["request"]["subtype"] == "can_use_tool"
                && value["request"]["tool_name"] == "Bash" =>
        {
            &value["request"]["input"]["command"]
        }
        _ => return false,
    };
    command.as_str().is_some_and(|s| !s.trim().is_empty())
}

fn permission_key(
    agent: AgentId,
    value: &Value,
    run: &str,
    cwd: &std::path::Path,
) -> Option<String> {
    if !command_request(agent, value) {
        return None;
    }
    let mut params = if agent == AgentId::Codex {
        value["params"].clone()
    } else {
        value["request"].clone()
    };
    // Ignore only routing identifiers. Keep every other field (including cwd,
    // tool input and requested permissions), so changed privileges require consent.
    let fields: &[&str] = if agent == AgentId::Codex {
        &["threadId", "turnId", "itemId", "approvalId"]
    } else {
        &["tool_use_id"]
    };
    for field in fields {
        params.as_object_mut()?.remove(*field);
    }
    Some(json!([run, cwd, agent, params]).to_string())
}

fn request(agent: AgentId, value: &Value) -> Result<Interaction, String> {
    if agent == AgentId::Codex && value["method"] == "mcpServer/elicitation/request" {
        return super::elicitation::request(value);
    }
    let p = if agent == AgentId::Codex {
        &value["params"]
    } else {
        &value["request"]
    };
    let method = value["method"].as_str().unwrap_or("");
    let tool = p["tool_name"].as_str().unwrap_or("");
    let questions = if agent == AgentId::Codex && method == "item/tool/requestUserInput" {
        &p["questions"]
    } else if agent == AgentId::Claude && tool == "AskUserQuestion" {
        &p["input"]["questions"]
    } else {
        &Value::Null
    };
    let approval = questions.is_null();
    if approval
        && !matches!(
            method,
            "item/commandExecution/requestApproval"
                | "item/fileChange/requestApproval"
                | "item/permissions/requestApproval"
        )
        && !(agent == AgentId::Claude && p["subtype"] == "can_use_tool")
    {
        return Err(format!(
            "不支持的交互请求：{}",
            if agent == AgentId::Codex {
                method
            } else {
                p["subtype"].as_str().unwrap_or("缺少 subtype")
            }
        ));
    }
    if !approval
        && questions
            .as_array()
            .is_none_or(|qs| qs.is_empty() || qs.len() > 32)
    {
        return Err("交互问题列表为空或超过限制".into());
    }
    let questions = questions
        .as_array()
        .map(|qs| {
            qs.iter()
                .map(|q| InteractionQuestion {
                    id: q[if agent == AgentId::Codex {
                        "id"
                    } else {
                        "question"
                    }]
                    .as_str()
                    .unwrap_or("")
                    .into(),
                    text: q["question"].as_str().unwrap_or("").into(),
                    options: q["options"]
                        .as_array()
                        .map(|os| {
                            os.iter()
                                .filter_map(|o| o["label"].as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                    multiple: q["multiSelect"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Interaction {
        protocol_id: None,
        kind: if approval { "permission" } else { "question" }.into(),
        mcp_mode: None,
        mcp_schema: None,
        mcp_url: None,
        mcp_context: None,
        policy: Default::default(),
        decision: None,
        delivery: None,
        id: uuid::Uuid::new_v4().to_string(),
        title: if approval {
            format!("请求授权：{}", if tool.is_empty() { method } else { tool })
        } else {
            "请选择或填写回答".into()
        },
        detail: serde_json::to_string_pretty(p).unwrap_or_default(),
        questions,
        approval,
        can_remember: command_request(agent, value),
        status: "pending".into(),
    })
}
fn response(
    agent: AgentId,
    raw: &Value,
    request: &Interaction,
    answer: &Value,
) -> Result<Value, String> {
    if request.kind.starts_with("mcp_") {
        return super::elicitation::response(raw, request, answer);
    }
    if let Some(remember) = answer.get("remember") {
        if !remember.is_boolean()
            || (remember == true
                && (!request.approval
                    || !request.can_remember
                    || !command_request(agent, raw)
                    || answer["decision"] != "allow"))
        {
            return Err("仅可在允许命令执行时记住该命令".into());
        }
    }
    let allow = answer["decision"].as_str() == Some("allow");
    if request.approval && !matches!(answer["decision"].as_str(), Some("allow" | "deny")) {
        return Err("请选择允许或拒绝".into());
    }
    let mut answers = serde_json::Map::new();
    if !request.approval {
        for q in &request.questions {
            let values = answer["answers"][&q.id]
                .as_array()
                .ok_or("请回答所有问题")?;
            if values.is_empty()
                || (!q.multiple && values.len() != 1)
                || values.iter().any(|v| {
                    v.as_str()
                        .is_none_or(|s| s.trim().is_empty() || s.len() > 16000)
                })
            {
                return Err("回答格式或长度无效".into());
            }
            answers.insert(
                q.id.clone(),
                if agent == AgentId::Codex {
                    json!({"answers":values})
                } else {
                    Value::String(
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", "),
                    )
                },
            );
        }
    }
    if agent == AgentId::Codex {
        let result = if !request.approval {
            json!({"answers":answers})
        } else if raw["method"] == "item/permissions/requestApproval" {
            json!({"permissions":if allow { raw["params"]["permissions"].clone() } else { json!({}) }, "scope":"turn"})
        } else {
            json!({"decision":if allow { "accept" } else { "decline" }})
        };
        Ok(json!({"id":raw["id"],"result":result}))
    } else {
        let mut input = raw["request"]["input"].clone();
        if !request.approval {
            input["answers"] = Value::Object(answers);
        }
        let result = if allow || !request.approval {
            json!({"behavior":"allow","updatedInput":input})
        } else {
            json!({"behavior":"deny","message":"用户拒绝了本次请求"})
        };
        Ok(
            json!({"type":"control_response","response":{"subtype":"success","request_id":raw["request_id"],"response":result}}),
        )
    }
}
async fn write(
    input: &mut (impl tokio::io::AsyncWrite + Unpin),
    value: &Value,
) -> Result<(), String> {
    input
        .write_all(format!("{value}\n").as_bytes())
        .await
        .map_err(|e| format!("写入 Agent 失败：{e}"))?;
    input
        .flush()
        .await
        .map_err(|e| format!("刷新 Agent 通道失败：{e}"))
}
pub async fn run<F: Fn(ParsedOutput) -> Result<(), String>>(
    spec: &RunSpec,
    config: &Config,
    cancel: oneshot::Receiver<()>,
    output: F,
) -> Result<i32, RunError> {
    run_with_timeout(spec, config, cancel, output, super::thinking_timeout::LIMIT).await
}

async fn run_with_timeout<F: Fn(ParsedOutput) -> Result<(), String>>(
    spec: &RunSpec,
    config: &Config,
    mut cancel: oneshot::Receiver<()>,
    output: F,
    thinking_limit: std::time::Duration,
) -> Result<i32, RunError> {
    let watchdog = super::thinking_timeout::Watchdog::new(thinking_limit);
    let deadline = super::thinking_timeout::Watchdog::new(spec.deadline);
    let _cleanup = Cleanup(config);
    if !matches!(cancel.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
        return Err(RunError::Cancelled);
    }
    let mut cmd = super::hidden_command(&spec.executable)?;
    super::supervision::configure(&mut cmd);
    let mut child = cmd
        .args(&spec.args)
        .envs(&spec.env)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| RunError::Failed(format!("启动失败：{e}")))?;
    let tree = match super::supervision::Tree::attach(&child) {
        Ok(t) => t,
        Err(e) => {
            super::terminate(&mut child).await;
            return Err(e.into());
        }
    };
    let mut input = child.stdin.take().ok_or("缺少 stdin".to_owned())?;
    let mut stdout = framing::Pipe::new(
        child.stdout.take().ok_or("缺少 stdout".to_owned())?,
        framing::AGENT_FRAME_LIMIT,
        false,
    );
    let mut stderr = framing::Pipe::new(
        child.stderr.take().ok_or("缺少 stderr".to_owned())?,
        1024 * 1024,
        true,
    );
    let (sender, mut replies) = mpsc::channel::<Reply>(16);
    let mut pending: HashMap<String, (Value, Interaction)> = HashMap::new();
    let mut seen_requests = HashSet::new();
    let mut active_thread = config.session.clone();
    let codex = spec.agent == AgentId::Codex;
    let work = async {
        write(&mut input, &if codex { json!({"id":"init","method":"initialize","params":{"clientInfo":{"name":"binweave","version":"0.1.0"},"capabilities":{"experimentalApi":true,"requiredHumanApproval":true}}}) } else { json!({"type":"control_request","request_id":"init","request":{"subtype":"initialize","hooks":{}}}) }).await?;
        let mut stderr_open = true;
        let mut total = 0usize;
        let mut policy_denials = 0usize;
        loop {
            // Buffered pipes can remain ready indefinitely. Yield so cancellation
            // reaches the outer select even during a flood of output/SQLite writes.
            tokio::task::yield_now().await;
            tokio::select! {
                reply = replies.recv() => {
                    let Some(reply) = reply else { return Err("交互回复通道已关闭".to_owned()); };
                    let Some((raw, request)) = pending.get(&reply.id) else { let _ = reply.ack.send(Err("交互已过期或重复回答".into())); continue; };
                    if config.analyst && request.approval && reply.answer["decision"] == "allow" && !super::read_policy::may_ask(spec.agent, raw, &spec.cwd) {
                        let _ = reply.ack.send(Err("读取路径或权限已变化，请拒绝请求并检查工作区".into()));
                        continue;
                    }
                    let wire = match response(spec.agent, raw, request, &reply.answer) { Ok(v) => v, Err(e) => { let _ = reply.ack.send(Err(e)); continue; } };
                    let remember = if reply.answer["remember"] == true { permission_key(spec.agent, raw, &config.run_id, &spec.cwd) } else { None };
                    let mut resolved = request.clone();
                    let mcp = resolved.kind.starts_with("mcp_");
                    if mcp { resolved.decision = reply.answer["action"].as_str().map(str::to_owned); }
                    if let Err(e) = write(&mut input, &wire).await {
                        if mcp {
                            resolved.status = "expired".into(); resolved.delivery = Some("failed".into());
                            let mut event = parser::parse(spec.agent, "", false); event.interaction = Some(resolved); output(event)?;
                        }
                        let _ = reply.ack.send(Err(e.clone())); return Err(e);
                    }
                    pending.remove(&reply.id);
                    config.broker.pending.lock().map_err(|_| "交互锁异常")?.remove(&reply.id);
                    let mut event = parser::parse(spec.agent, "", false);
                    if mcp {
                        resolved.status = "answered".into(); resolved.delivery = Some("delivered".into());
                        event.interaction = Some(resolved);
                    } else { event.resolved_interaction = Some(reply.id); }
                    let saved = output(event).and_then(|()| {
                        if let Some(key) = remember { config.broker.remembered.lock().map_err(|_| "授权记录锁异常")?.insert(key); }
                        Ok(())
                    });
                    let _ = reply.ack.send(saved.clone());
                    saved?;
                    if pending.is_empty() { watchdog.progress(); } else { watchdog.pause(); }
                    if mcp && !pending.values().any(|(_, r)| r.kind.starts_with("mcp_")) { deadline.resume_budget(); }
                }
                line = stderr.next(), if stderr_open => {
                    match line? { Some(line) => { total += line.len(); output(parser::parse(spec.agent, &line, true))?; }, None => stderr_open = false }
                }
                line = stdout.next() => {
                    let Some(line) = line? else { return Ok(false); };
                    total += line.len();
                    if total > 256 * 1024 * 1024 { return Err("Agent 输出超过本轮 256 MiB 限制".into()); }
                    let parsed = parser::parse(spec.agent, &line, false);
                    if pending.is_empty() && super::thinking_timeout::is_progress(spec.agent, &parsed) { watchdog.progress(); }
                    output(parsed)?;
                    let Ok(value) = serde_json::from_str::<Value>(&line) else { continue; };
                    let class = super::elicitation::classify(&value);
                    if codex && class == super::elicitation::Inbound::Malformed { return Err("Codex 入站协议消息格式无效：请检查 CLI 版本与 JSON-RPC 字段".into()); }
                    let client_response = codex && class == super::elicitation::Inbound::Response;
                    if (client_response && value.get("error").is_some()) || (!codex && value["type"] == "control_response" && value["response"]["subtype"] == "error") { return Err(format!("协议错误：{value}")); }
                    if (client_response && value["id"] == "init") || (!codex && value["type"] == "control_response" && value["response"]["request_id"] == "init") {
                        if codex {
                            write(&mut input, &json!({"method":"initialized"})).await?;
                            let mut params = json!({"cwd":spec.cwd,"approvalPolicy":"on-request","sandbox":if config.analyst {"read-only"} else {"workspace-write"}});
                            if let Some(session) = &config.session { params["threadId"] = json!(session); }
                            write(&mut input, &json!({"id":"thread","method":if config.session.is_some() {"thread/resume"} else {"thread/start"},"params":params})).await?;
                        } else { write(&mut input, &json!({"type":"user","session_id":"","message":{"role":"user","content":spec.input},"parent_tool_use_id":null})).await?; }
                    } else if client_response && value["id"] == "thread" {
                        let thread = value["result"]["thread"]["id"].as_str().ok_or("缺少 Codex thread ID")?;
                        active_thread = Some(thread.to_owned());
                        output(parser::parse(spec.agent, &json!({"type":"thread.started","thread_id":thread}).to_string(), false))?;
                        write(&mut input, &json!({"id":"turn","method":"turn/start","params":{"threadId":thread,"input":[{"type":"text","text":spec.input,"text_elements":[]}]}})).await?;
                    } else if (codex && class == super::elicitation::Inbound::Request) || (!codex && value["type"] == "control_request") {
                        if pending.len() >= 64 { return Err("待处理交互超过 64 个，请检查服务端后恢复".into()); }
                        if codex && !seen_requests.insert(value["id"].to_string()) { return Err("服务端重复使用请求 ID；已停止以避免重复投递".into()); }
                        let mut request = request(spec.agent, &value)?;
                        if request.kind.starts_with("mcp_") {
                            if active_thread.as_deref().is_some_and(|thread| value["params"]["threadId"].as_str() != Some(thread)) {
                                return Err("MCP 请求不属于当前 Codex 会话，已停止并保留恢复检查点".into());
                            }
                            request.policy = config.elicitation_policy;
                            if config.elicitation_policy != crate::model::ElicitationPolicy::Ask {
                                let action = if config.elicitation_policy == crate::model::ElicitationPolicy::Decline { "decline" } else { "cancel" };
                                let mut event = parser::parse(spec.agent, "", false); event.interaction = Some(request.clone()); output(event)?;
                                let wire = response(spec.agent, &value, &request, &json!({"action":action}))?;
                                request.decision = Some(action.into());
                                let result = write(&mut input, &wire).await;
                                request.status = if result.is_ok() { "answered" } else { "expired" }.into();
                                request.delivery = Some(if result.is_ok() { "delivered" } else { "failed" }.into());
                                let mut event = parser::parse(spec.agent, "", false); event.interaction = Some(request); output(event)?;
                                result?;
                                continue;
                            }
                            deadline.pause_budget();
                        }
                        // A remembered implementer grant cannot elevate an analyst.
                        if config.analyst && request.approval && !super::read_policy::may_ask(spec.agent, &value, &spec.cwd) {
                            let mut wire = response(spec.agent, &value, &request, &json!({"decision":"deny"}))?;
                            if !codex { wire["response"]["response"]["message"] = json!("分析方读取边界拒绝。不要重复请求或绕过；改用现有 REA 反编译、反汇编与结构证据，最多一次合规替代。仍缺少原始指针字节时提交 ANALYSIS_DEGRADED 并记录 open_uncertainties，或请求人工补证。"); }
                            write(&mut input, &wire).await?;
                            request.status = "blocked".into();
                            request.title = format!("分析方权限边界已拒绝（{:?}）：{}", spec.agent, request.title);
                            let mut event = parser::parse(spec.agent, "", false);
                            event.interaction = Some(request);
                            output(event)?;
                            policy_denials += 1;
                            if policy_denials >= 2 { return Err(format!("READ_POLICY_BLOCKED：{:?} 连续请求超出分析方权限边界，已停止重复尝试。请人工补证或调整合规读取方式后单侧重试。", spec.agent)); }
                            continue;
                        }
                        if config.analyst { request.can_remember = false; }
                        let key = if config.analyst { None } else { permission_key(spec.agent, &value, &config.run_id, &spec.cwd) };
                        let remembered = key.as_ref().is_some_and(|key| config.broker.remembered.lock().is_ok_and(|keys| keys.contains(key)));
                        if remembered {
                            let wire = response(spec.agent, &value, &request, &json!({"decision":"allow"}))?;
                            write(&mut input, &wire).await?;
                            request.status = "answered".into();
                            request.title = format!("已按记住的命令自动允许：{}", request.title);
                            let mut event = parser::parse(spec.agent, "", false);
                            event.interaction = Some(request);
                            output(event)?;
                            if pending.is_empty() { watchdog.progress(); }
                            continue;
                        }
                        config.broker.pending.lock().map_err(|_| "交互锁异常")?.insert(request.id.clone(), (config.run_id.clone(), config.prompt_id.clone(), sender.clone()));
                        pending.insert(request.id.clone(), (value.clone(), request.clone()));
                        watchdog.pause();
                        let mut event = parser::parse(spec.agent, "", false); event.interaction = Some(request); output(event)?;
                    } else if !codex && value["type"] == "control_cancel_request" {
                        return Err("Agent 撤回了交互请求，请检查当前输出后恢复。".into());
                    }
                    if codex && value["method"] == "item/completed" {
                        let item = &value["params"]["item"];
                        if item["type"] == "agentMessage" { output(parser::parse(spec.agent, &json!({"type":"item.completed","item":{"type":"agent_message","id":item["id"],"text":item["text"],"phase":item["phase"]}}).to_string(), false))?; }
                    }
                    if codex && value["method"] == "turn/completed" {
                        if !pending.is_empty() { return Err("Agent 在交互回答前结束轮次；请求已失效，请人工检查后恢复".into()); }
                        let ok = value["params"]["turn"]["status"] == "completed";
                        output(parser::parse(spec.agent, &json!({"type":if ok {"turn.completed"} else {"turn.failed"},"error":value["params"]["turn"]["error"]}).to_string(), false))?;
                        return Ok(true);
                    }
                    if !codex && value["type"] == "result" { return Ok(true); }
                }
            }
            if total > 256 * 1024 * 1024 {
                return Err("Agent 输出超过本轮 256 MiB 限制".into());
            }
        }
    };
    let result = tokio::select! { biased;
        _ = &mut cancel => Err(RunError::Cancelled),
        _ = deadline.expired() => Err(RunError::TimedOut),
        _ = watchdog.expired() => Err(RunError::ThinkingTimedOut),
        result = work => result.map_err(RunError::Failed),
    };
    // app-server is long lived: its turn completion, not process exit, ends this invocation.
    drop(input);
    let waiting_for_exit = result.is_ok();
    let result = tokio::select! { biased;
      _ = &mut cancel, if waiting_for_exit => Err(RunError::Cancelled),
      result = async { match result {
        Ok(true) if codex => Ok(0),
        Ok(true) => {
            match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
                Ok(status) => status
                    .map(|s| s.code().unwrap_or(-1))
                    .map_err(|e| RunError::Failed(e.to_string())),
                Err(_) => Err(RunError::Failed("Agent 已返回结果但未完成退出清理".into())),
            }
        }
        Ok(false) => {
            match tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await {
                Ok(status) => status
                    .map(|s| s.code().unwrap_or(-1))
                    .map_err(|e| RunError::Failed(e.to_string())),
                Err(_) => Err(RunError::Failed("Agent 关闭输出后未退出".into())),
            }
        }
        Err(e) => Err(e),
      }} => result,
    };
    drop(tree);
    super::terminate(&mut child).await;
    result
}

#[cfg(test)]
#[path = "interactive_tests.rs"]
mod tests;
