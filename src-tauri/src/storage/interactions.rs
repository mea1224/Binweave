use super::*;

impl Store {
    pub fn set_elicitation_policy(
        &self,
        id: &str,
        policy: ElicitationPolicy,
    ) -> Result<RunSnapshot, String> {
        let mut db = self.db()?;
        let mut s = load(&db, id)?;
        if s.run.execution.chain_active {
            return Err("请先停止执行再修改 MCP 交互策略".into());
        }
        s.run.execution.elicitation_policy = policy;
        let name = match policy {
            ElicitationPolicy::Ask => "询问用户",
            ElicitationPolicy::Decline => "无人值守拒绝",
            ElicitationPolicy::Cancel => "无人值守取消",
        };
        push_event(
            &mut s,
            None,
            EventKind::System,
            &format!("MCP 交互策略已更新为 {name}，仅对后续轮次生效。"),
            EdgeType::Sequence,
        );
        save(&mut db, &s)?;
        Ok(s)
    }
    pub fn interaction(
        &self,
        id: &str,
        prompt_id: &str,
        request: Option<Interaction>,
        resolved: Option<&str>,
    ) -> Result<RunSnapshot, String> {
        let mut db = self.db()?;
        let mut s = load(&db, id)?;
        let turn = s.run.execution.turns.last_mut().ok_or("缺少当前执行轮次")?;
        if turn.prompt_id != prompt_id
            || !s.run.execution.chain_active
            || !matches!(turn.status, TurnStatus::Submitted | TurnStatus::Running)
        {
            return Err("交互已过期，不能回复已结束的任务".into());
        }
        let agent = turn.agent;
        let detail = if let Some(request) = request {
            let detail = if request.kind.starts_with("mcp_") && request.status != "pending" {
                format!(
                    "{}：选择 {}；投递状态 {}（未保存表单回答）。",
                    request.title,
                    request.decision.as_deref().unwrap_or("无"),
                    request.delivery.as_deref().unwrap_or("未知")
                )
            } else if request.status != "pending" {
                request.title.clone()
            } else {
                format!("等待用户处理：{}", request.title)
            };
            if let Some(existing) = turn.interactions.iter_mut().find(|r| r.id == request.id) {
                if existing.status != "pending" || request.status == "pending" {
                    return Err("交互已回复或重复请求".into());
                }
                *existing = request;
            } else {
                turn.interactions.push(request);
            }
            detail
        } else {
            let request = turn
                .interactions
                .iter_mut()
                .find(|r| Some(r.id.as_str()) == resolved && r.status == "pending")
                .ok_or("交互已回复或已过期")?;
            request.status = "answered".into();
            "交互响应已写入 Agent 通道，等待后续输出。".into()
        };
        push_event(
            &mut s,
            Some(agent),
            EventKind::System,
            &detail,
            EdgeType::Sequence,
        );
        save(&mut db, &s)?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mcp_pending_requests_expire_on_restart_and_policy_survives() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let s = store
            .ensure_active_run(&dir.path().to_string_lossy())
            .unwrap();
        store
            .set_elicitation_policy(&s.run.id, ElicitationPolicy::Cancel)
            .unwrap();
        let s = store
            .begin_prompt(&s.run.id, AgentId::Codex, "任务", None)
            .unwrap();
        let r: Interaction = serde_json::from_value(serde_json::json!({"id":"route","protocolId":"\"thread\"","title":"MCP","detail":"确认","questions":[],"approval":false,"kind":"mcp_confirmation","status":"pending","delivery":"pending","policy":"ask"})).unwrap();
        let turn = &s.run.execution.turns[0].prompt_id;
        store
            .interaction(&s.run.id, turn, Some(r.clone()), None)
            .unwrap();
        assert!(store.interaction(&s.run.id, turn, Some(r), None).is_err());
        store.recover_interrupted().unwrap();
        let saved = Store::open(dir.path())
            .unwrap()
            .snapshot(&s.run.id)
            .unwrap();
        assert_eq!(
            saved.run.execution.elicitation_policy,
            ElicitationPolicy::Cancel
        );
        assert_eq!(
            saved.run.execution.turns[0].interactions[0]
                .delivery
                .as_deref(),
            Some("expired")
        );
        assert_eq!(
            saved.run.execution.turns[0].interactions[0].status,
            "expired"
        );
        assert!(store
            .interaction(&s.run.id, turn, None, Some("route"))
            .is_err());
    }
    #[test]
    fn interactions_persist_and_expire_without_creating_turns_or_replaying_work() {
        for agent in [AgentId::Codex, AgentId::Claude] {
            for restart in [true, false] {
                let dir = tempfile::tempdir().unwrap();
                let store = Store::open(dir.path()).unwrap();
                let s = store
                    .ensure_active_run(&dir.path().to_string_lossy())
                    .unwrap();
                let s = store.begin_prompt(&s.run.id, agent, "任务", None).unwrap();
                let turn = &s.run.execution.turns[0].prompt_id;
                let r = Interaction {
                    protocol_id: None,
                    kind: "permission".into(),
                    mcp_mode: None,
                    mcp_schema: None,
                    mcp_url: None,
                    mcp_context: None,
                    policy: Default::default(),
                    decision: None,
                    delivery: None,
                    id: "req".into(),
                    title: "授权".into(),
                    detail: "原始请求".into(),
                    questions: vec![],
                    approval: true,
                    can_remember: true,
                    status: "pending".into(),
                };
                assert!(store
                    .interaction(&s.run.id, "wrong", Some(r.clone()), None)
                    .is_err());
                store.interaction(&s.run.id, turn, Some(r), None).unwrap();
                let blocked = Interaction {
                    protocol_id: None,
                    kind: "permission".into(),
                    mcp_mode: None,
                    mcp_schema: None,
                    mcp_url: None,
                    mcp_context: None,
                    policy: Default::default(),
                    decision: None,
                    delivery: None,
                    id: "blocked".into(),
                    title: "权限策略拒绝".into(),
                    detail: "读取 ../outside 被拒绝".into(),
                    questions: vec![],
                    approval: true,
                    can_remember: false,
                    status: "blocked".into(),
                };
                store
                    .interaction(&s.run.id, turn, Some(blocked), None)
                    .unwrap();
                if restart {
                    store.recover_interrupted().unwrap();
                } else {
                    store
                        .finish_prompt(&s.run.id, agent, false, "错误代码 TEST")
                        .unwrap();
                }
                assert!(store
                    .interaction(&s.run.id, turn, None, Some("req"))
                    .is_err());
                let saved = Store::open(dir.path())
                    .unwrap()
                    .snapshot(&s.run.id)
                    .unwrap();
                assert_eq!(saved.run.execution.turns.len(), 1);
                assert_eq!(
                    saved.run.execution.turns[0].interactions[1].status,
                    "blocked"
                );
                assert_eq!(
                    saved.run.execution.turns[0].interactions[1].detail,
                    "读取 ../outside 被拒绝"
                );
                assert_eq!(
                    saved.run.execution.turns[0].interactions[0].status,
                    "expired"
                );
                assert_eq!(
                    saved.run.execution.turns[0].interactions[0].detail,
                    "原始请求"
                );
            }
        }
    }
}
