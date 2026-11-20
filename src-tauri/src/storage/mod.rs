use crate::{model::*, orchestrator};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};
use uuid::Uuid;

mod history;
mod interactions;
#[cfg(test)]
mod recovery_tests;
mod skills;
mod workspaces;

pub struct Store {
    pub directory: PathBuf,
    connection: Mutex<Connection>,
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl Store {
    pub fn open(directory: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(directory).map_err(err)?;
        let db = Connection::open(directory.join("binweave.sqlite3")).map_err(err)?;
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(err)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY, updated_at TEXT NOT NULL, state TEXT NOT NULL, payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS run_checkpoints(run_id TEXT PRIMARY KEY REFERENCES sessions(id), payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS run_events(id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES sessions(id), sequence INTEGER NOT NULL, payload TEXT NOT NULL, UNIQUE(run_id, sequence));
            CREATE TABLE IF NOT EXISTS run_edges(id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES sessions(id), source TEXT NOT NULL REFERENCES run_events(id), target TEXT NOT NULL REFERENCES run_events(id), payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS run_artifacts(id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES sessions(id), payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS workspace_settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            PRAGMA user_version=1;").map_err(err)?;
        Ok(Self {
            directory: directory.to_path_buf(),
            connection: Mutex::new(db),
        })
    }
    fn db(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.connection
            .lock()
            .map_err(|_| "Database lock is poisoned".into())
    }

    pub fn ensure_active_run(&self, workspace: &str) -> Result<RunSnapshot, String> {
        let mut db = self.db()?;
        let id: Option<String> = db.query_row("SELECT id FROM sessions WHERE state NOT IN ('completed','failed') ORDER BY updated_at DESC LIMIT 1", [], |r| r.get(0)).optional().map_err(err)?;
        if let Some(id) = id {
            let snapshot = load(&db, &id)?;
            self.export_artifacts(&snapshot)?;
            return Ok(snapshot);
        }
        self.create_run(&mut db, workspace)
    }

    fn create_run(&self, db: &mut Connection, workspace: &str) -> Result<RunSnapshot, String> {
        if Path::new(workspace).is_absolute() && Path::new(workspace).is_dir() {
            crate::role_skills::initialize(Path::new(workspace))?;
        }
        let now = Utc::now();
        let id = Uuid::new_v4().to_string();
        let run = Run {
            id: id.clone(),
            title: "New analysis run".into(),
            workspace: workspace.into(),
            created_at: now,
            updated_at: now,
            state: RunState::Ready,
            roles: RoleMapping::default(),
            execution: Execution::default(),
        };
        let checkpoint = Checkpoint {
            logical_role: None,
            provider: None,
            run_id: id,
            state: RunState::Ready,
            active_agent: None,
            next_step: "Review diagnostics and role contract".into(),
            last_event_id: None,
            updated_at: now,
            role_revision: 1,
            interrupted_state: None,
        };
        let mut snapshot = RunSnapshot {
            run,
            checkpoint,
            events: vec![],
            edges: vec![],
            artifacts: vec![],
        };
        push_event(
            &mut snapshot,
            None,
            EventKind::System,
            "Workspace initialized. No agent has been launched.",
            EdgeType::Sequence,
        );
        add_contract(&mut snapshot);
        save(db, &snapshot)?;
        self.export_artifacts(&snapshot)?;
        Ok(snapshot)
    }
    pub fn snapshot(&self, id: &str) -> Result<RunSnapshot, String> {
        let db = self.db()?;
        load(&db, id)
    }

    pub fn append_output(
        &self,
        id: &str,
        agent: AgentId,
        output: &crate::process::parser::ParsedOutput,
    ) -> Result<RunSnapshot, String> {
        self.append_outputs(id, agent, std::slice::from_ref(output))
    }

    pub fn append_outputs(
        &self,
        id: &str,
        agent: AgentId,
        outputs: &[crate::process::parser::ParsedOutput],
    ) -> Result<RunSnapshot, String> {
        let mut db = self.db()?;
        let mut snapshot = load(&db, id)?;
        let event_count = snapshot.events.len();
        let edge_count = snapshot.edges.len();
        for output in outputs {
            if let Some(session) = &output.session_id {
                // CLI session identifiers are UUIDs. Reject option-like or malformed values.
                if uuid::Uuid::parse_str(session).is_err() {
                    return Err("CLI 返回的会话标识格式无效，已停止执行。".into());
                }
                if uuid::Uuid::parse_str(session).is_ok() {
                    let slot = match agent {
                        AgentId::Codex => &mut snapshot.run.execution.codex_session,
                        AgentId::Claude => &mut snapshot.run.execution.claude_session,
                    };
                    if slot.as_ref().is_some_and(|existing| existing != session) {
                        return Err("CLI 返回了不同的会话标识，已停止执行，未切换会话。".into());
                    }
                    *slot = Some(session.clone());
                }
            }
            if let Some(turn) = snapshot.run.execution.turns.last_mut() {
                if turn.agent == agent && turn.status == TurnStatus::Submitted {
                    turn.status = TurnStatus::Running;
                }
            }
            let raw_kind = if output.kind == EventKind::Stderr {
                EventKind::Stderr
            } else {
                EventKind::Stdout
            };
            push_event(
                &mut snapshot,
                Some(agent),
                raw_kind,
                &output.raw,
                EdgeType::Sequence,
            );
            if let Some(summary) = &output.summary {
                push_event(
                    &mut snapshot,
                    Some(agent),
                    output.kind,
                    summary,
                    EdgeType::Sequence,
                );
            }
        }
        save_suffix(
            &mut db,
            &snapshot,
            event_count,
            edge_count,
            snapshot.artifacts.len(),
        )?;
        Ok(snapshot)
    }

    pub fn begin_prompt(
        &self,
        id: &str,
        agent: AgentId,
        content: &str,
        source_id: Option<&str>,
    ) -> Result<RunSnapshot, String> {
        self.begin_prompt_with_requirements(id, agent, content, source_id, None)
    }

    pub fn begin_prompt_with_requirements(
        &self,
        id: &str,
        agent: AgentId,
        content: &str,
        source_id: Option<&str>,
        requirements: Option<&str>,
    ) -> Result<RunSnapshot, String> {
        self.begin_turn(
            id,
            agent,
            content,
            source_id,
            false,
            requirements,
            None,
            None,
        )
    }

    pub fn begin_continuation(
        &self,
        id: &str,
        agent: AgentId,
        content: &str,
        task_id: &str,
    ) -> Result<RunSnapshot, String> {
        self.begin_turn(id, agent, content, None, false, None, Some(task_id), None)
    }

    pub fn begin_auto_prompt(
        &self,
        id: &str,
        agent: AgentId,
        content: &str,
        source_id: &str,
    ) -> Result<RunSnapshot, String> {
        self.begin_turn(id, agent, content, Some(source_id), true, None, None, None)
    }

    pub fn restart_turn(
        &self,
        id: &str,
        agent: AgentId,
        prompt_id: &str,
    ) -> Result<RunSnapshot, String> {
        self.begin_turn(
            id,
            agent,
            "单侧恢复",
            None,
            false,
            None,
            None,
            Some(prompt_id),
        )
    }

    fn begin_turn(
        &self,
        id: &str,
        agent: AgentId,
        content: &str,
        source_id: Option<&str>,
        automatic: bool,
        requirements: Option<&str>,
        continuation_id: Option<&str>,
        restart_id: Option<&str>,
    ) -> Result<RunSnapshot, String> {
        if content.trim().is_empty() || content.len() > 65_536 {
            return Err("提示词需包含 1–65536 字节的非空内容".into());
        }
        let requirements = requirements.filter(|text| !text.trim().is_empty());
        if requirements.is_some() && (source_id.is_none() || automatic) {
            return Err("补充要求仅用于手动交接".into());
        }
        let mut combined = match requirements {
            Some(text) => format!("{content}\n\n用户补充要求：\n{text}"),
            None => content.to_owned(),
        };
        if combined.len() > 65_536 {
            return Err("交接原文与补充要求合计不能超过 65536 字节".into());
        }
        let mut db = self.db()?;
        let mut s = load(&db, id)?;
        if s.run.state != RunState::Ready && restart_id.is_none() {
            return Err("请等待当前执行结束，或先确认恢复检查点".into());
        }
        if let Some(prompt_id) = restart_id {
            if s.run.execution.chain_active
                || !matches!(
                    s.run.state,
                    RunState::Ready | RunState::Paused | RunState::Interrupted
                )
            {
                return Err("请先停止并等待当前执行链清理结束".into());
            }
            let turn = s
                .run
                .execution
                .turns
                .iter()
                .find(|t| t.agent == agent && t.prompt_id == prompt_id)
                .ok_or("此侧没有可恢复的任务")?;
            // Recovery attempts remain in their original card. The final card
            // is the last turn that did not originate from a recovery edge.
            let latest_card = s
                .run
                .execution
                .turns
                .iter()
                .rev()
                .find(|t| {
                    !s.edges
                        .iter()
                        .any(|e| e.edge_type == EdgeType::Recovery && e.target == t.prompt_id)
                })
                .ok_or("没有可重试的会话")?;
            let mut root = prompt_id;
            for _ in 0..s.run.execution.turns.len() {
                match s
                    .edges
                    .iter()
                    .find(|e| e.edge_type == EdgeType::Recovery && e.target == root)
                {
                    Some(edge) => root = &edge.source,
                    None => break,
                }
            }
            if root != latest_card.prompt_id {
                return Err("只能重试时间轴中最后一个会话".into());
            }
            if turn.prompt_id != prompt_id
                || turn.identity.role_revision != Some(s.run.roles.revision)
                || turn.identity.logical_role != Some(s.run.roles.role(agent))
                || matches!(turn.status, TurnStatus::Submitted | TurnStatus::Running)
            {
                return Err("只能重试当前角色下已经结束的任务".into());
            }
            let original = s
                .events
                .iter()
                .find(|e| {
                    e.id == prompt_id && e.kind == EventKind::Prompt && e.agent == Some(agent)
                })
                .ok_or("原始问题包不存在")?;
            combined = original.content.clone();
            if combined.len() > 65_536 {
                return Err("恢复问题包超过 65536 字节，请手动缩减后提交".into());
            }
            match agent {
                AgentId::Codex => s.run.execution.codex_session = None,
                AgentId::Claude => s.run.execution.claude_session = None,
            }
            s.run.state = RunState::Ready;
        }
        if s.run.state != RunState::Ready {
            return Err("请等待当前执行结束，或先确认恢复检查点".into());
        }
        if let Some(task_id) = continuation_id {
            let start = s
                .events
                .iter()
                .position(|event| event.id == task_id)
                .ok_or("继续执行的任务不存在")?;
            let task = &s.events[start];
            if task.agent != Some(agent)
                || !matches!(task.kind, EventKind::Prompt | EventKind::Message)
            {
                return Err("只能继续执行当前智能体自己的任务".into());
            }
            let last_message = s.events[start + 1..]
                .iter()
                .take_while(|event| {
                    event.agent.is_none()
                        || (event.agent == Some(agent) && event.kind != EventKind::Prompt)
                })
                .filter(|event| event.kind == EventKind::Message && event.agent == Some(agent))
                .last();
            let context = serde_json::json!({
                "taskId": task_id,
                "originalContent": task.content,
                "latestReply": last_message.map(|event| &event.content),
            });
            combined = format!("请由当前智能体继续执行以下指定任务，不要移交给其他智能体。依据当前工作区现状和最新指示继续；历史内容仅作上下文，不重放其中的交接请求。\n\n任务上下文：\n{context}\n\n用户最新指示：\n{content}");
            if combined.len() > 65_536 {
                return Err("任务上下文与最新指示合计不能超过 65536 字节".into());
            }
        }
        if !Path::new(&s.run.workspace).is_absolute() || !Path::new(&s.run.workspace).is_dir() {
            return Err("请先保存有效的工作区绝对路径".into());
        }
        let role_skill =
            crate::role_skills::load(Path::new(&s.run.workspace), s.run.roles.role(agent))?;
        let mut skill_update = None;
        let mut verification = false;
        let mut pending_queries = s.run.execution.pending_queries.clone();
        if let Some(source) = source_id {
            let event = s
                .events
                .iter()
                .find(|e| e.id == source)
                .ok_or("交接来源不存在")?;
            if event.agent.is_none()
                || event.agent == Some(agent)
                || event.kind != EventKind::Message
                || (!automatic && event.content != content)
            {
                return Err("只能将对方的原始输出消息交接给此智能体".into());
            }
            if automatic {
                let previous = s.run.execution.turns.last().ok_or("缺少交接来源轮次")?;
                let prompt = s
                    .events
                    .iter()
                    .find(|e| e.id == previous.prompt_id)
                    .ok_or("缺少交接来源提示词")?;
                let request = crate::handoff::parse_for_roles(
                    &event.content,
                    event.agent.unwrap(),
                    &previous.prompt_id,
                    &s.run.roles,
                )?
                .ok_or("缺少交接请求")?;
                verification = agent == s.run.roles.analyst;
                if verification {
                    pending_queries = crate::handoff::query_ids(&event.content);
                } else {
                    crate::handoff::validate_answers(&event.content, &pending_queries)?;
                    if crate::handoff::is_degraded(&event.content) {
                        return Err("降级结果须经人工审阅，不自动派发实现或清除待查问题".into());
                    }
                    pending_queries.clear();
                }
                if previous.status != TurnStatus::Completed
                    || previous.agent != event.agent.unwrap()
                    || event.sequence <= prompt.sequence
                    || request.to != agent
                    || request.instruction != content
                {
                    return Err("交接来源未完成或指令不一致".into());
                }
                if request.evidence_ids.iter().any(|id| {
                    !s.events.iter().any(|e| e.id == *id)
                        && !s.artifacts.iter().any(|a| a.id == *id)
                }) {
                    return Err("交接引用的证据标识不存在，已停止自动派发".into());
                }
                if let Some(update) = request.skill_update {
                    if event.agent != Some(s.run.roles.analyst)
                        || agent != s.run.roles.implementer
                        || update.base_artifact_id != s.run.execution.active_skill_id
                    {
                        return Err("技能更新仅允许当前分析侧提交，且必须基于当前生效版本".into());
                    }
                    skill_update = Some(update);
                }
            }
            if !automatic && crate::handoff::is_degraded(&event.content) {
                combined = format!("操作员手动转交降级分析供审阅。保留 open_uncertainties；语义证据不等于原始字节验证，不得据未核实字节实现，也不得自动重复派发同一读取问题。需要人工补证时停止并报告。\n\n{combined}");
                if combined.len() > 65_536 {
                    return Err("降级交接内容超过长度限制".into());
                }
            }
            if s.edges.iter().any(|e| {
                e.source == source
                    && matches!(e.edge_type, EdgeType::Handoff | EdgeType::Verification)
            }) {
                return Err("此消息已经交接，请勿重复发送".into());
            }
        }
        if let Some(update) = skill_update {
            skills::add_task_skill(&mut s, source_id.unwrap(), &update.content);
        }
        s.run.state = if verification {
            RunState::Verifying
        } else if s.run.roles.analyst == agent {
            RunState::Analyzing
        } else {
            RunState::Implementing
        };
        s.checkpoint.state = s.run.state;
        s.checkpoint.active_agent = Some(agent);
        s.checkpoint.logical_role = Some(s.run.roles.role(agent));
        s.checkpoint.provider = Some(agent);
        s.checkpoint.interrupted_state = None;
        s.checkpoint.next_step = "提示词已接受，正在启动本轮工作进程；输出将持续保存。".into();
        push_event(
            &mut s,
            Some(agent),
            EventKind::Prompt,
            &combined,
            EdgeType::Sequence,
        );
        if let Some(source) = source_id.or(continuation_id).or(restart_id) {
            let target = s.events.last_mut().unwrap();
            target.parent_id = Some(source.into());
            let target_id = target.id.clone();
            s.edges.retain(|edge| edge.target != target_id);
            s.edges.push(CausalEdge {
                identity: s.run.roles.stamp(Some(agent)),
                id: Uuid::new_v4().to_string(),
                run_id: id.into(),
                source: source.into(),
                target: target_id,
                edge_type: if restart_id.is_some() {
                    EdgeType::Recovery
                } else if continuation_id.is_some() {
                    EdgeType::Sequence
                } else if verification {
                    EdgeType::Verification
                } else {
                    EdgeType::Handoff
                },
            });
        }
        s.run.execution.chain_active = true;
        s.run.execution.pending_queries = pending_queries;
        s.run.execution.turns.push(Turn {
            identity: s.run.roles.stamp(Some(agent)),
            role_skill: Some(role_skill),
            prompt_id: s.events.last().unwrap().id.clone(),
            agent,
            status: TurnStatus::Submitted,
            detail: "已提交，等待 CLI 输出".into(),
            interactions: vec![],
        });
        // Export before accepting the destination turn. A failed export never queues work.
        // An unreferenced file after a failed DB commit is not an accepted skill revision.
        self.export_artifacts(&s)?;
        if let Some(skill) = s
            .run
            .execution
            .active_skill_id
            .as_ref()
            .and_then(|id| s.artifacts.iter().find(|a| &a.id == id))
        {
            crate::role_skills::archive_task(
                Path::new(&s.run.workspace),
                &skill.id,
                &skill.content,
            )?;
        }
        save(&mut db, &s)?;
        Ok(s)
    }

    pub fn finish_prompt(
        &self,
        id: &str,
        agent: AgentId,
        success: bool,
        detail: &str,
    ) -> Result<RunSnapshot, String> {
        let mut db = self.db()?;
        let mut s = load(&db, id)?;
        if !orchestrator::is_running(s.run.state) || s.checkpoint.active_agent != Some(agent) {
            return Err("工作进程与检查点不一致".into());
        }
        s.run.state = if success {
            RunState::Ready
        } else {
            RunState::Paused
        };
        s.checkpoint.state = s.run.state;
        s.checkpoint.active_agent = None;
        s.checkpoint.next_step = detail.into();
        if let Some(turn) = s.run.execution.turns.last_mut() {
            turn.status = if success {
                TurnStatus::Completed
            } else if turn.interactions.iter().any(|r| {
                r.kind.starts_with("mcp_")
                    && r.delivery.as_deref() == Some("delivered")
                    && matches!(r.decision.as_deref(), Some("decline" | "cancel"))
            }) {
                TurnStatus::Cancelled
            } else {
                TurnStatus::Failed
            };
            turn.detail = detail.into();
            for request in &mut turn.interactions {
                if request.status == "pending" {
                    request.status = "expired".into();
                    if request.kind.starts_with("mcp_") {
                        request.delivery = Some("expired".into());
                    }
                }
            }
        }
        push_event(
            &mut s,
            Some(agent),
            EventKind::System,
            detail,
            EdgeType::Sequence,
        );
        save(&mut db, &s)?;
        Ok(s)
    }

    pub fn end_chain(
        &self,
        id: &str,
        pause: Option<&str>,
        cancelled: bool,
    ) -> Result<RunSnapshot, String> {
        let mut db = self.db()?;
        let mut s = load(&db, id)?;
        s.run.execution.chain_active = false;
        if let Some(detail) = pause {
            s.run.state = RunState::Paused;
            s.checkpoint.state = RunState::Paused;
            s.checkpoint.active_agent = None;
            s.checkpoint.next_step = detail.into();
            if cancelled {
                if let Some(turn) = s.run.execution.turns.last_mut() {
                    if turn.status != TurnStatus::Completed {
                        turn.status = TurnStatus::Cancelled;
                        turn.detail = detail.into();
                    }
                }
            }
            push_event(&mut s, None, EventKind::System, detail, EdgeType::Sequence);
        } else {
            push_event(
                &mut s,
                None,
                EventKind::System,
                "本次执行链已结束。",
                EdgeType::Sequence,
            );
        }
        save(&mut db, &s)?;
        Ok(s)
    }

    pub fn reset_session(&self, id: &str, agent: AgentId) -> Result<RunSnapshot, String> {
        let mut db = self.db()?;
        let mut s = load(&db, id)?;
        if !matches!(
            s.run.state,
            RunState::Ready | RunState::Paused | RunState::Interrupted
        ) || s.run.execution.chain_active
        {
            return Err("请先停止执行并确认检查点".into());
        }
        match agent {
            AgentId::Codex => s.run.execution.codex_session = None,
            AgentId::Claude => s.run.execution.claude_session = None,
        }
        push_event(
            &mut s,
            Some(agent),
            EventKind::System,
            "已清除接续标识；下一条提示词将开始新会话，历史记录保留。",
            EdgeType::Sequence,
        );
        save(&mut db, &s)?;
        Ok(s)
    }

    // Legacy note fixtures exercise preservation/deletion of pre-existing data.
    // New notes are no longer supported by the production application.
    #[cfg(test)]
    fn append_note(&self, id: &str, content: &str) -> Result<RunSnapshot, String> {
        let content = content.trim();
        if content.is_empty() || content.len() > 16_384 {
            return Err("A note must contain 1–16384 bytes.".into());
        }
        let mut db = self.db()?;
        let mut snapshot = load(&db, id)?;
        push_event(
            &mut snapshot,
            None,
            EventKind::Message,
            content,
            EdgeType::Sequence,
        );
        save(&mut db, &snapshot)?;
        Ok(snapshot)
    }
    pub fn set_workspace(&self, id: &str, workspace: &str) -> Result<RunSnapshot, String> {
        let path = Path::new(workspace);
        if !path.is_absolute() || !path.is_dir() {
            return Err("Workspace must be an existing absolute directory".into());
        }
        let path = path
            .canonicalize()
            .map_err(|_| "Unable to resolve workspace directory")?;
        let mut db = self.db()?;
        let mut snapshot = load(&db, id)?;
        workspaces::require_idle(&snapshot)?;
        crate::role_skills::initialize(&path)?;
        snapshot.run.workspace = path.to_string_lossy().into_owned();
        snapshot.run.execution.active_skill_id = None;
        snapshot.run.execution.pending_queries.clear();
        snapshot.run.execution.codex_session = None;
        snapshot.run.execution.claude_session = None;
        push_event(
            &mut snapshot,
            None,
            EventKind::System,
            "Workspace changed by the operator; diagnostics must be rerun.",
            EdgeType::Sequence,
        );
        save(&mut db, &snapshot)?;
        Ok(snapshot)
    }
    pub fn swap_roles(&self, id: &str) -> Result<RunSnapshot, String> {
        let mut db = self.db()?;
        let mut snapshot = load(&db, id)?;
        workspaces::require_idle(&snapshot)?;
        crate::role_skills::initialize(Path::new(&snapshot.run.workspace))?;
        if !matches!(snapshot.run.state, RunState::Ready | RunState::Paused) {
            return Err("Roles can be swapped only while ready or paused.".into());
        }
        let roles = &mut snapshot.run.roles;
        std::mem::swap(&mut roles.analyst, &mut roles.implementer);
        roles.revision += 1;
        snapshot.checkpoint.role_revision = roles.revision;
        snapshot.checkpoint.active_agent = None;
        snapshot.checkpoint.logical_role = None;
        snapshot.checkpoint.provider = None;
        // Task evidence belongs to logical roles and survives provider swaps.
        // Start fresh CLI sessions so obsolete role instructions cannot linger.
        snapshot.run.execution.codex_session = None;
        snapshot.run.execution.claude_session = None;
        push_event(
            &mut snapshot,
            None,
            EventKind::System,
            "Analyst and implementer roles swapped by the operator.",
            EdgeType::Handoff,
        );
        add_contract(&mut snapshot);
        save(&mut db, &snapshot)?;
        self.export_artifacts(&snapshot)?;
        Ok(snapshot)
    }
    pub fn acknowledge_recovery(&self, id: &str) -> Result<RunSnapshot, String> {
        let mut db = self.db()?;
        let mut snapshot = load(&db, id)?;
        if snapshot.run.execution.chain_active {
            return Err("请等待执行链清理结束".into());
        }
        if !matches!(snapshot.run.state, RunState::Paused | RunState::Interrupted) {
            return Err("当前任务无需确认恢复检查点".into());
        }
        snapshot.run.state = RunState::Ready;
        snapshot.checkpoint.state = RunState::Ready;
        snapshot.checkpoint.active_agent = None;
        snapshot.checkpoint.interrupted_state = None;
        snapshot.checkpoint.next_step =
            "已确认检查点，可继续发送指令；会话已保留，未重放旧任务。".into();
        push_event(
            &mut snapshot,
            None,
            EventKind::StateTransition,
            "已确认检查点并恢复就绪；会话已保留，未重放旧任务。",
            EdgeType::Recovery,
        );
        save(&mut db, &snapshot)?;
        Ok(snapshot)
    }

    pub fn transition(&self, id: &str, next: RunState, step: &str) -> Result<RunSnapshot, String> {
        let mut db = self.db()?;
        let mut snapshot = load(&db, id)?;
        let previous = snapshot.run.state;
        if !orchestrator::can_transition(previous, next) {
            return Err(format!("Invalid transition: {previous:?} -> {next:?}"));
        }
        snapshot.run.state = next;
        snapshot.checkpoint.state = next;
        if matches!(next, RunState::Paused | RunState::Interrupted) {
            snapshot.checkpoint.interrupted_state = Some(previous);
        } else {
            snapshot.checkpoint.interrupted_state = None;
            snapshot.checkpoint.active_agent =
                orchestrator::active_agent(next, &snapshot.run.roles);
        }
        snapshot.checkpoint.next_step = step.into();
        let edge = if next == RunState::Interrupted {
            EdgeType::Recovery
        } else {
            EdgeType::Sequence
        };
        let agent = snapshot.checkpoint.active_agent;
        push_event(
            &mut snapshot,
            agent,
            EventKind::StateTransition,
            &format!("{previous:?} -> {next:?}: {step}"),
            edge,
        );
        save(&mut db, &snapshot)?;
        Ok(snapshot)
    }
    pub fn recover_interrupted(&self) -> Result<usize, String> {
        let ids: Vec<String> = {
            let db = self.db()?;
            let mut stmt = db.prepare("SELECT id FROM sessions WHERE state IN ('analyzing','implementing','verifying','updating_skill') OR json_extract(payload, '$.execution.chainActive') = 1").map_err(err)?;
            let values = stmt
                .query_map([], |r| r.get(0))
                .map_err(err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(err)?;
            values
        };
        for id in &ids {
            let mut db = self.db()?;
            let mut snapshot = load(&db, id)?;
            snapshot.checkpoint.interrupted_state = Some(snapshot.run.state);
            snapshot.run.state = RunState::Interrupted;
            snapshot.checkpoint.state = RunState::Interrupted;
            snapshot.run.execution.chain_active = false;
            if let Some(turn) = snapshot.run.execution.turns.last_mut() {
                if matches!(turn.status, TurnStatus::Submitted | TurnStatus::Running) {
                    turn.status = TurnStatus::Interrupted;
                    turn.detail = "应用退出导致执行中断，未自动重放。".into();
                    for request in &mut turn.interactions {
                        if request.status == "pending" {
                            request.status = "expired".into();
                            if request.kind.starts_with("mcp_") {
                                request.delivery = Some("expired".into());
                            }
                        }
                    }
                }
            }
            snapshot.checkpoint.next_step =
                "执行或交接期间应用退出，请核对输出和文件修改；未自动重放任何指令。".into();
            push_event(
                &mut snapshot,
                None,
                EventKind::System,
                "发现中断的执行链，已暂停并保留会话与交接记录。",
                EdgeType::Recovery,
            );
            save(&mut db, &snapshot)?;
        }
        Ok(ids.len())
    }
    fn export_artifacts(&self, snapshot: &RunSnapshot) -> Result<(), String> {
        for artifact in &snapshot.artifacts {
            let relative = Path::new(&artifact.relative_path);
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                return Err("Unsafe artifact path in database".into());
            }
            let path = self.directory.join(relative);
            let parent = path.parent().ok_or("Artifact directory is missing")?;
            std::fs::create_dir_all(parent).map_err(err)?;
            if std::fs::read_to_string(&path).ok().as_deref() != Some(&artifact.content) {
                std::fs::write(&path, &artifact.content).map_err(|e| {
                    format!("技能产物导出失败；已提交的产物将在下次启动时重试导出：{e}")
                })?;
            }
        }
        Ok(())
    }
}

fn push_event(
    s: &mut RunSnapshot,
    agent: Option<AgentId>,
    kind: EventKind,
    content: &str,
    edge_type: EdgeType,
) {
    let now = Utc::now();
    let id = Uuid::new_v4().to_string();
    let parent = s.checkpoint.last_event_id.clone();
    if let Some(active) = s.checkpoint.active_agent {
        s.checkpoint.logical_role = Some(s.run.roles.role(active));
        s.checkpoint.provider = Some(active);
    }
    if let Some(source) = &parent {
        s.edges.push(CausalEdge {
            identity: s.run.roles.stamp(agent),
            id: Uuid::new_v4().to_string(),
            run_id: s.run.id.clone(),
            source: source.clone(),
            target: id.clone(),
            edge_type,
        });
    }
    s.events.push(AgentEvent {
        identity: s.run.roles.stamp(agent),
        id: id.clone(),
        run_id: s.run.id.clone(),
        sequence: s.events.last().map_or(1, |e| e.sequence + 1),
        created_at: now,
        agent,
        kind,
        content: content.into(),
        parent_id: parent,
    });
    s.checkpoint.last_event_id = Some(id);
    s.checkpoint.updated_at = now;
    s.run.updated_at = now;
}
fn add_contract(s: &mut RunSnapshot) {
    let revision = s.run.roles.revision;
    let path = format!("artifacts/{}/skills/v{revision}/SKILL.md", s.run.id);
    s.artifacts.push(Artifact {
        id: Uuid::new_v4().to_string(),
        run_id: s.run.id.clone(),
        revision,
        relative_path: path.clone(),
        content: orchestrator::skill_contract(&s.run.roles),
        created_at: Utc::now(),
    });
    push_event(
        s,
        None,
        EventKind::SkillUpdate,
        &format!("Role contract v{revision} saved: {path}"),
        EdgeType::SkillRevision,
    );
}
fn json(value: &impl serde::Serialize) -> Result<String, String> {
    serde_json::to_string(value).map_err(err)
}
fn load(db: &Connection, id: &str) -> Result<RunSnapshot, String> {
    let run: String = db
        .query_row("SELECT payload FROM sessions WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .map_err(err)?;
    let checkpoint: String = db
        .query_row(
            "SELECT payload FROM run_checkpoints WHERE run_id=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(err)?;
    Ok(RunSnapshot {
        run: serde_json::from_str(&run).map_err(err)?,
        checkpoint: serde_json::from_str(&checkpoint).map_err(err)?,
        events: load_list(
            db,
            "SELECT payload FROM run_events WHERE run_id=?1 ORDER BY sequence",
            id,
        )?,
        edges: load_list(
            db,
            "SELECT payload FROM run_edges WHERE run_id=?1 ORDER BY rowid",
            id,
        )?,
        artifacts: load_list(
            db,
            "SELECT payload FROM run_artifacts WHERE run_id=?1 ORDER BY rowid",
            id,
        )?,
    })
}
fn load_list<T: serde::de::DeserializeOwned>(
    db: &Connection,
    sql: &str,
    id: &str,
) -> Result<Vec<T>, String> {
    let mut stmt = db.prepare(sql).map_err(err)?;
    let rows = stmt
        .query_map([id], |r| r.get::<_, String>(0))
        .map_err(err)?;
    rows.map(|row| serde_json::from_str(&row.map_err(err)?).map_err(err))
        .collect()
}
fn save(db: &mut Connection, s: &RunSnapshot) -> Result<(), String> {
    save_suffix(db, s, 0, 0, 0)
}

// Output is append-only: commit just the new records and the current checkpoint.
// Do not serialize and attempt to reinsert the entire history for every token.
fn save_suffix(
    db: &mut Connection,
    s: &RunSnapshot,
    events: usize,
    edges: usize,
    artifacts: usize,
) -> Result<(), String> {
    let tx = db.transaction().map_err(err)?;
    let state = serde_json::to_value(s.run.state).map_err(err)?;
    tx.execute("INSERT INTO sessions VALUES (?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET updated_at=excluded.updated_at,state=excluded.state,payload=excluded.payload", params![s.run.id, s.run.updated_at.to_rfc3339(), state.as_str(), json(&s.run)?]).map_err(err)?;
    tx.execute("INSERT INTO run_checkpoints VALUES (?1,?2) ON CONFLICT(run_id) DO UPDATE SET payload=excluded.payload", params![s.run.id, json(&s.checkpoint)?]).map_err(err)?;
    for e in &s.events[events..] {
        tx.execute(
            "INSERT INTO run_events VALUES (?1,?2,?3,?4) ON CONFLICT(id) DO NOTHING",
            params![e.id, e.run_id, e.sequence, json(e)?],
        )
        .map_err(err)?;
    }
    for e in &s.edges[edges..] {
        tx.execute(
            "INSERT INTO run_edges VALUES (?1,?2,?3,?4,?5) ON CONFLICT(id) DO NOTHING",
            params![e.id, e.run_id, e.source, e.target, json(e)?],
        )
        .map_err(err)?;
    }
    for a in &s.artifacts[artifacts..] {
        tx.execute(
            "INSERT INTO run_artifacts VALUES (?1,?2,?3) ON CONFLICT(id) DO NOTHING",
            params![a.id, a.run_id, json(a)?],
        )
        .map_err(err)?;
    }
    tx.commit().map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn role_identity_and_loaded_skill_survive_swap_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let initial = store
            .ensure_active_run(&dir.path().to_string_lossy())
            .unwrap();
        let id = &initial.run.id;
        let sent = store
            .begin_prompt(id, AgentId::Codex, "分析", None)
            .unwrap();
        let first = sent.run.execution.turns[0].clone();
        assert_eq!(first.identity.logical_role, Some(LogicalRole::Analyst));
        assert!(
            crate::worker::turn_spec(&sent, AgentId::Codex, "分析", true)
                .args
                .contains(&"read-only".into())
        );
        assert!(store.swap_roles(id).is_err());
        store
            .finish_prompt(id, AgentId::Codex, true, "完成")
            .unwrap();
        store.end_chain(id, None, false).unwrap();
        let swapped = store.swap_roles(id).unwrap();
        assert_eq!(
            swapped.run.execution.turns[0].identity.role_revision,
            Some(1)
        );
        assert_eq!(
            swapped
                .events
                .iter()
                .find(|e| e.id == first.prompt_id)
                .unwrap()
                .identity
                .logical_role,
            Some(LogicalRole::Analyst)
        );
        std::fs::write(dir.path().join(".rule/analyst/SKILL.md"), "新分析技能").unwrap();
        let sent = store
            .begin_prompt(id, AgentId::Claude, "查证", None)
            .unwrap();
        let current = sent.run.execution.turns.last().unwrap();
        assert_eq!(current.identity.logical_role, Some(LogicalRole::Analyst));
        assert_eq!(current.identity.provider, Some(AgentId::Claude));
        assert_eq!(current.identity.role_revision, Some(2));
        assert_eq!(current.role_skill.as_ref().unwrap().content, "新分析技能");
        let spec = crate::worker::turn_spec(&sent, AgentId::Claude, "查证", true);
        assert!(spec.input.unwrap().contains("新分析技能"));
        assert!(spec.args.contains(&"--disallowedTools".into()));
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        store.recover_interrupted().unwrap();
        let restored = store.snapshot(id).unwrap();
        assert_eq!(restored.checkpoint.logical_role, Some(LogicalRole::Analyst));
        assert_eq!(restored.checkpoint.provider, Some(AgentId::Claude));
        assert_eq!(
            restored.run.execution.turns[0]
                .role_skill
                .as_ref()
                .unwrap()
                .content,
            first.role_skill.unwrap().content
        );
        let old: Turn = serde_json::from_value(serde_json::json!({"promptId":"old","agent":"codex","status":"completed","detail":"旧数据"})).unwrap();
        assert!(old.identity.logical_role.is_none());
        assert!(old.role_skill.is_none());
    }

    #[test]
    fn blocked_requests_persist_verification_route_and_require_complete_answers() {
        for swapped in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(dir.path()).unwrap();
            let initial = store
                .ensure_active_run(&dir.path().to_string_lossy())
                .unwrap();
            let id = &initial.run.id;
            let initial = if swapped {
                store.swap_roles(id).unwrap()
            } else {
                initial.clone()
            };
            let roles = &initial.run.roles;
            let sent = store
                .begin_prompt(id, roles.implementer, "实现", None)
                .unwrap();
            let turn = &sent.run.execution.turns.last().unwrap().prompt_id;
            let text = serde_json::json!({"turnId":turn,"logical_role":"implementer","handoff_type":"BLOCKED_LOGIC","summary":"需要查证","stopped_at":"parser","questions":[{"id":"Q1","target":"header","question":"偏移？","why_needed":"读取字段"}]}).to_string();
            let saved = store
                .append_output(
                    id,
                    roles.implementer,
                    &crate::process::parser::ParsedOutput {
                        interaction: None,
                        resolved_interaction: None,
                        kind: EventKind::Message,
                        raw: "fixture".into(),
                        summary: Some(text.clone()),
                        session_id: None,
                        reported_failure: false,
                    },
                )
                .unwrap();
            let source = &saved.events.last().unwrap().id;
            store
                .finish_prompt(id, roles.implementer, true, "停止编写")
                .unwrap();
            let request = crate::handoff::parse_for_roles(&text, roles.implementer, turn, roles)
                .unwrap()
                .unwrap();
            let next = store
                .begin_auto_prompt(id, roles.analyst, &request.instruction, source)
                .unwrap();
            assert_eq!(next.run.state, RunState::Verifying);
            assert_eq!(next.run.execution.pending_queries, ["Q1"]);
            assert!(next.edges.iter().any(|e| e.source == *source
                && e.edge_type == EdgeType::Verification
                && e.identity.logical_role == Some(LogicalRole::Analyst)));
            store
                .finish_prompt(id, roles.analyst, true, "暂未查清")
                .unwrap();
            assert!(store
                .begin_auto_prompt(id, roles.analyst, &request.instruction, source)
                .is_err());
        }
    }
    #[test]
    fn sessions_survive_restart_and_reset_on_workspace_change() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let initial = store
            .ensure_active_run(&dir.path().to_string_lossy())
            .unwrap();
        let id = &initial.run.id;
        let codex = Uuid::new_v4().to_string();
        let claude = Uuid::new_v4().to_string();
        for (agent, session) in [(AgentId::Codex, &codex), (AgentId::Claude, &claude)] {
            store.begin_prompt(id, agent, "执行", None).unwrap();
            store
                .append_output(
                    id,
                    agent,
                    &crate::process::parser::ParsedOutput {
                        interaction: None,
                        resolved_interaction: None,
                        kind: EventKind::Stdout,
                        raw: "session".into(),
                        summary: None,
                        reported_failure: false,
                        session_id: Some(session.clone()),
                    },
                )
                .unwrap();
            store.finish_prompt(id, agent, true, "完成").unwrap();
        }
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.recover_interrupted().unwrap(), 1); // crash between turns
        let saved = store.snapshot(id).unwrap();
        assert_eq!(saved.run.state, RunState::Interrupted);
        assert_eq!(
            saved.run.execution.session(AgentId::Codex),
            Some(codex.as_str())
        );
        assert_eq!(
            saved.run.execution.session(AgentId::Claude),
            Some(claude.as_str())
        );
        assert!(!saved.run.execution.chain_active);
        store.reset_session(id, AgentId::Codex).unwrap();
        assert_eq!(
            store.snapshot(id).unwrap().run.execution.claude_session,
            Some(claude)
        );
        store.transition(id, RunState::Ready, "确认恢复").unwrap();
        let changed = store
            .set_workspace(id, &dir.path().to_string_lossy())
            .unwrap();
        assert!(changed.run.execution.codex_session.is_none());
        assert!(changed.run.execution.claude_session.is_none());
        assert_eq!(changed.run.execution.turns.len(), 2);
    }

    #[test]
    fn legacy_runs_default_to_empty_execution() {
        let value = serde_json::json!({"id":"old", "title":"", "workspace":"", "createdAt":Utc::now(), "updatedAt":Utc::now(), "state":"ready", "roles":RoleMapping::default()});
        let run: Run = serde_json::from_value(value).unwrap();
        assert!(!run.execution.chain_active);
        assert!(run.execution.turns.is_empty());
    }
    #[test]
    fn prompts_route_by_identity_and_handoffs_persist_bidirectionally() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let s = store
            .ensure_active_run(&dir.path().to_string_lossy())
            .unwrap();
        let id = &s.run.id;
        store.swap_roles(id).unwrap();
        let sent = store
            .begin_prompt(id, AgentId::Codex, "  原始提示词  ", None)
            .unwrap();
        assert_eq!(sent.checkpoint.active_agent, Some(AgentId::Codex));
        assert_eq!(sent.run.state, RunState::Implementing);
        assert_eq!(sent.events.last().unwrap().content, "  原始提示词  ");
        assert!(store
            .begin_prompt(id, AgentId::Claude, "duplicate", None)
            .is_err());
        for (source, target) in [
            (AgentId::Codex, AgentId::Claude),
            (AgentId::Claude, AgentId::Codex),
        ] {
            let output = crate::process::parser::ParsedOutput {
                interaction: None,
                resolved_interaction: None,
                kind: EventKind::Message,
                raw: "raw".into(),
                summary: Some("交接原文".into()),
                session_id: None,
                reported_failure: false,
            };
            let out = store.append_output(id, source, &output).unwrap();
            let source_id = &out.events.last().unwrap().id;
            store.finish_prompt(id, source, true, "完成").unwrap();
            assert!(store
                .begin_prompt(id, source, "交接原文", Some(source_id))
                .is_err());
            assert!(store
                .begin_prompt(id, target, "tampered", Some(source_id))
                .is_err());
            let handed = store
                .begin_prompt(id, target, "交接原文", Some(source_id))
                .unwrap();
            assert_eq!(
                handed.events.last().unwrap().parent_id.as_ref(),
                Some(source_id)
            );
            assert!(handed
                .edges
                .iter()
                .any(|e| e.source == *source_id && e.edge_type == EdgeType::Handoff));
        }
        store
            .finish_prompt(id, AgentId::Codex, false, "失败")
            .unwrap();
        assert_eq!(store.snapshot(id).unwrap().run.state, RunState::Paused);
        store.transition(id, RunState::Ready, "确认").unwrap();
        let restored = Store::open(dir.path()).unwrap().snapshot(id).unwrap();
        let edge = restored
            .edges
            .iter()
            .find(|e| {
                e.edge_type == EdgeType::Handoff
                    && restored
                        .events
                        .iter()
                        .any(|v| v.id == e.source && v.agent == Some(AgentId::Codex))
            })
            .unwrap();
        assert!(store
            .begin_prompt(id, AgentId::Claude, "交接原文", Some(&edge.source))
            .is_err());
        assert!(store.begin_prompt(id, AgentId::Codex, "  ", None).is_err());
        assert!(store
            .begin_prompt(id, AgentId::Codex, &"x".repeat(65537), None)
            .is_err());
    }
    #[test]
    fn manual_handoff_requirements_preserve_source_and_persist_combined_prompt() {
        for requirements in [None, Some(" \n\t"), Some("  只修改测试，保留原有接口。  ")]
        {
            for source in [AgentId::Codex, AgentId::Claude] {
                let target = if source == AgentId::Codex {
                    AgentId::Claude
                } else {
                    AgentId::Codex
                };
                let dir = tempfile::tempdir().unwrap();
                let store = Store::open(dir.path()).unwrap();
                let s = store
                    .ensure_active_run(&dir.path().to_string_lossy())
                    .unwrap();
                let id = &s.run.id;
                store.begin_prompt(id, source, "开始", None).unwrap();
                let output = crate::process::parser::ParsedOutput {
                    interaction: None,
                    resolved_interaction: None,
                    kind: EventKind::Message,
                    raw: "raw".into(),
                    summary: Some("  交接原文\n".into()),
                    session_id: None,
                    reported_failure: false,
                };
                let out = store.append_output(id, source, &output).unwrap();
                let event = out.events.last().unwrap();
                store.finish_prompt(id, source, true, "完成").unwrap();
                assert!(store
                    .begin_prompt_with_requirements(
                        id,
                        target,
                        "被篡改的原文",
                        Some(&event.id),
                        requirements
                    )
                    .is_err());
                assert!(store
                    .begin_prompt_with_requirements(
                        id,
                        target,
                        &event.content,
                        Some(&event.id),
                        Some(&"大".repeat(65536))
                    )
                    .is_err());
                assert!(store
                    .begin_prompt_with_requirements(id, target, &event.content, None, Some("要求"))
                    .is_err());
                let handed = store
                    .begin_prompt_with_requirements(
                        id,
                        target,
                        &event.content,
                        Some(&event.id),
                        requirements,
                    )
                    .unwrap();
                let expected = match requirements.filter(|s| !s.trim().is_empty()) {
                    Some(text) => format!("{}\n\n用户补充要求：\n{text}", event.content),
                    None => event.content.clone(),
                };
                let prompt = handed.events.last().unwrap();
                assert_eq!(prompt.content, expected);
                assert_eq!(prompt.parent_id.as_deref(), Some(event.id.as_str()));
                assert_eq!(
                    handed
                        .events
                        .iter()
                        .find(|e| e.id == event.id)
                        .unwrap()
                        .content,
                    event.content
                );
                assert!(handed.edges.iter().any(|e| e.source == event.id
                    && e.target == prompt.id
                    && e.edge_type == EdgeType::Handoff));
                store.finish_prompt(id, target, true, "完成").unwrap();
                let restored = Store::open(dir.path()).unwrap().snapshot(id).unwrap();
                assert_eq!(
                    restored
                        .events
                        .iter()
                        .find(|e| e.id == prompt.id)
                        .unwrap()
                        .content,
                    expected
                );
                assert!(store
                    .begin_prompt_with_requirements(
                        id,
                        target,
                        &event.content,
                        Some(&event.id),
                        requirements
                    )
                    .is_err());
            }
        }
    }
    #[test]
    fn continuation_keeps_selected_task_context_and_agent_without_handoff() {
        for agent in [AgentId::Codex, AgentId::Claude] {
            let other = if agent == AgentId::Codex {
                AgentId::Claude
            } else {
                AgentId::Codex
            };
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(dir.path()).unwrap();
            let s = store
                .ensure_active_run(&dir.path().to_string_lossy())
                .unwrap();
            let id = &s.run.id;
            let first = store
                .begin_prompt(id, agent, "原任务：检查边界", None)
                .unwrap();
            let task_id = &first.events.last().unwrap().id;
            assert!(store
                .begin_continuation(id, agent, "追加要求", task_id)
                .is_err());
            let output = crate::process::parser::ParsedOutput {
                interaction: None,
                resolved_interaction: None,
                kind: EventKind::Message,
                raw: "raw".into(),
                summary: Some("该任务的最新回复".into()),
                session_id: Some("12345678-1234-4234-8234-123456789abc".into()),
                reported_failure: false,
            };
            store.append_output(id, agent, &output).unwrap();
            store.finish_prompt(id, agent, true, "完成").unwrap();
            // Later tasks must not replace the selected card's context.
            store.begin_prompt(id, agent, "另一个任务", None).unwrap();
            let later = crate::process::parser::ParsedOutput {
                summary: Some("不相关的新任务回复".into()),
                ..output
            };
            store.append_output(id, agent, &later).unwrap();
            store.finish_prompt(id, agent, true, "完成").unwrap();
            assert!(store
                .begin_continuation(id, other, "要求", task_id)
                .is_err());
            assert!(store
                .begin_continuation(id, agent, "要求", "missing")
                .is_err());
            assert!(store.begin_continuation(id, agent, " \n", task_id).is_err());
            assert!(store
                .begin_continuation(id, agent, &"a".repeat(65536), task_id)
                .is_err());
            let continued = store
                .begin_continuation(id, agent, "  请补充回归测试\n保留接口  ", task_id)
                .unwrap();
            let prompt = continued.events.last().unwrap();
            assert_eq!(prompt.agent, Some(agent));
            assert_eq!(prompt.parent_id.as_deref(), Some(task_id.as_str()));
            assert!(prompt.content.contains("原任务：检查边界"));
            assert!(prompt.content.contains("该任务的最新回复"));
            assert!(!prompt.content.contains("不相关的新任务回复"));
            assert!(prompt.content.ends_with("  请补充回归测试\n保留接口  "));
            assert_eq!(
                continued.run.execution.session(agent),
                Some("12345678-1234-4234-8234-123456789abc")
            );
            assert!(continued.edges.iter().any(|edge| edge.source == *task_id
                && edge.target == prompt.id
                && edge.edge_type == EdgeType::Sequence));
            assert!(!continued
                .edges
                .iter()
                .any(|edge| matches!(edge.edge_type, EdgeType::Handoff | EdgeType::Verification)));
            store.finish_prompt(id, agent, true, "完成").unwrap();
            let restored = Store::open(dir.path()).unwrap().snapshot(id).unwrap();
            assert_eq!(
                restored
                    .events
                    .iter()
                    .find(|event| event.id == prompt.id)
                    .unwrap()
                    .content,
                prompt.content
            );
        }
    }
    #[test]
    fn persists_events_edges_and_role_contract() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let first = store
            .ensure_active_run(&dir.path().to_string_lossy())
            .unwrap();
        store.append_note(&first.run.id, "Operator note").unwrap();
        let swapped = store.swap_roles(&first.run.id).unwrap();
        assert_eq!(swapped.run.roles.analyst, AgentId::Claude);
        assert_eq!(swapped.checkpoint.role_revision, 2);
        assert_eq!(swapped.edges.len(), swapped.events.len() - 1);
        assert!(dir
            .path()
            .join(&swapped.artifacts[1].relative_path)
            .exists());
        drop(store);
        let reopened = Store::open(dir.path())
            .unwrap()
            .ensure_active_run("ignored")
            .unwrap();
        assert_eq!(reopened.run.id, first.run.id);
        assert_eq!(reopened.events.len(), swapped.events.len());
    }
    #[test]
    fn recovery_preserves_interrupted_side_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let s = store.ensure_active_run("workspace").unwrap();
        store
            .transition(&s.run.id, RunState::Analyzing, "Inspect target")
            .unwrap();
        assert!(store.swap_roles(&s.run.id).is_err());
        assert_eq!(store.recover_interrupted().unwrap(), 1);
        assert_eq!(store.recover_interrupted().unwrap(), 0);
        let s = store.snapshot(&s.run.id).unwrap();
        assert_eq!(s.checkpoint.interrupted_state, Some(RunState::Analyzing));
        assert_eq!(s.checkpoint.active_agent, Some(AgentId::Codex));
    }
    #[test]
    fn rejects_invalid_changes_without_events() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let s = store.ensure_active_run("workspace").unwrap();
        assert!(store
            .transition(&s.run.id, RunState::Completed, "Skip")
            .is_err());
        assert!(store.append_note(&s.run.id, "   ").is_err());
        assert_eq!(
            store.snapshot(&s.run.id).unwrap().events.len(),
            s.events.len()
        );
    }
    #[test]
    fn rolls_back_checkpoint_and_state_when_event_insert_fails() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let original = store.ensure_active_run("workspace").unwrap();
        store.db().unwrap().execute_batch("CREATE TRIGGER reject_transition BEFORE INSERT ON run_events WHEN NEW.sequence > 2 BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
        assert!(store
            .transition(&original.run.id, RunState::Analyzing, "Analyze")
            .is_err());
        let after = store.snapshot(&original.run.id).unwrap();
        assert_eq!(after.run.state, RunState::Ready);
        assert_eq!(after.checkpoint.state, RunState::Ready);
        assert_eq!(
            after.checkpoint.last_event_id,
            original.checkpoint.last_event_id
        );
        assert_eq!(after.events.len(), original.events.len());
        assert_eq!(after.edges.len(), original.edges.len());
    }
    #[test]
    fn restores_missing_artifact_from_committed_database() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let original = store.ensure_active_run("workspace").unwrap();
        let artifact = &original.artifacts[0];
        let path = dir.path().join(&artifact.relative_path);
        std::fs::remove_file(&path).unwrap();
        let restored = store.ensure_active_run("ignored").unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), artifact.content);
        assert_eq!(restored.events.len(), original.events.len());
    }
    #[test]
    fn workspace_and_recovery_controls_validate_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let original = store.ensure_active_run("workspace").unwrap();
        assert!(store.set_workspace(&original.run.id, "relative").is_err());
        let snapshot = store
            .set_workspace(&original.run.id, &dir.path().to_string_lossy())
            .unwrap();
        assert!(std::path::Path::new(&snapshot.run.workspace).is_absolute());
        store
            .transition(&original.run.id, RunState::Paused, "Paused")
            .unwrap();
        assert!(store
            .set_workspace(&original.run.id, &dir.path().to_string_lossy())
            .is_ok());
        let ready = store
            .transition(&original.run.id, RunState::Ready, "Acknowledged")
            .unwrap();
        assert_eq!(ready.checkpoint.active_agent, None);
        assert_eq!(ready.checkpoint.interrupted_state, None);
    }
    #[test]
    fn preserves_raw_output_and_derived_summary_with_causal_link() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let first = store.ensure_active_run("workspace").unwrap();
        let raw = r#"{"type":"item.completed","item":{"type":"agent_message","text":"原文保留"}}"#;
        let output = crate::process::parser::parse(AgentId::Codex, raw, false);
        let s = store
            .append_output(&first.run.id, AgentId::Codex, &output)
            .unwrap();
        assert_eq!(s.events[s.events.len() - 2].content, raw);
        assert_eq!(s.events.last().unwrap().content, "原文保留");
        assert_eq!(
            s.events.last().unwrap().parent_id.as_ref(),
            Some(&s.events[s.events.len() - 2].id)
        );
    }
    #[test]
    fn output_batch_preserves_order_and_rolls_back_as_one_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let first = store.ensure_active_run("workspace").unwrap();
        let outputs: Vec<_> = (0..100).map(|i| crate::process::parser::parse(AgentId::Codex, &serde_json::json!({"method":"item/agentMessage/delta","params":{"itemId":"m","delta":i.to_string()}}).to_string(), false)).collect();
        let saved = store
            .append_outputs(&first.run.id, AgentId::Codex, &outputs)
            .unwrap();
        assert_eq!(saved.events.len(), first.events.len() + 200);
        for (pair, output) in saved.events[first.events.len()..].chunks(2).zip(&outputs) {
            assert_eq!(pair[0].content, output.raw);
            assert_eq!(Some(&pair[0].id), pair[1].parent_id.as_ref());
            assert_eq!(pair[1].sequence, pair[0].sequence + 1);
        }
        let limit = saved.events.len() + 2;
        store.db().unwrap().execute_batch(&format!("CREATE TRIGGER reject_batch BEFORE INSERT ON run_events WHEN NEW.sequence > {limit} BEGIN SELECT RAISE(ABORT, 'batch failure'); END;")).unwrap();
        assert!(store
            .append_outputs(&first.run.id, AgentId::Codex, &outputs)
            .is_err());
        assert_eq!(
            store.snapshot(&first.run.id).unwrap().events.len(),
            saved.events.len()
        );
    }
}
