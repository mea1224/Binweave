use crate::{
    model::{AgentId, RunSnapshot},
    process::stream::{RunError, RunSpec},
    storage::Store,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::oneshot;

#[derive(Default)]
pub struct Workers(
    pub tokio::sync::Mutex<Option<(String, tokio::sync::watch::Sender<bool>)>>,
    pub Arc<crate::process::interactive::Broker>,
);

#[cfg(test)]
mod elicitation_tests {
    use super::*;
    use crate::model::{ElicitationPolicy, RunState, TurnStatus};
    use crate::process::interactive::{Broker, Config};

    #[tokio::test]
    async fn decisions_persist_and_stop_automatic_chaining_without_replaying_answers() {
        for (policy, action) in [
            (ElicitationPolicy::Ask, "accept"),
            (ElicitationPolicy::Ask, "decline"),
            (ElicitationPolicy::Ask, "cancel"),
            (ElicitationPolicy::Decline, "decline"),
            (ElicitationPolicy::Cancel, "cancel"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let store = Arc::new(Store::open(directory.path()).unwrap());
            let initial = store
                .ensure_active_run(&directory.path().to_string_lossy())
                .unwrap();
            let id = initial.run.id.clone();
            store.set_elicitation_policy(&id, policy).unwrap();
            let accepted = store
                .begin_prompt(&id, AgentId::Codex, "MCP 测试", None)
                .unwrap();
            assert!(store
                .set_elicitation_policy(&id, ElicitationPolicy::Ask)
                .is_err());
            let broker = Arc::new(Broker::default());
            let seen = Mutex::new(std::collections::HashSet::new());
            let replies = Mutex::new(Vec::new());
            let (_stop, cancel) = tokio::sync::watch::channel(false);
            execute_chain_with(
                store.clone(),
                accepted,
                AgentId::Codex,
                "测试".into(),
                true,
                cancel,
                |s| {
                    if policy == ElicitationPolicy::Ask {
                        for r in &s.run.execution.turns.last().unwrap().interactions {
                            if r.status == "pending" && seen.lock().unwrap().insert(r.id.clone()) {
                                let broker = broker.clone();
                                let id = id.clone();
                                let request_id = r.id.clone();
                                let turn = s.run.execution.turns.last().unwrap().prompt_id.clone();
                                replies.lock().unwrap().push(tokio::spawn(async move {
                                    broker
                                        .respond(
                                            &id,
                                            &turn,
                                            &request_id,
                                            serde_json::json!({"action":action,"content":{}}),
                                        )
                                        .await
                                        .unwrap();
                                }));
                            }
                        }
                    }
                },
                |s, _, _, _| RunSpec {
                    interactive: Some(Config {
                        elicitation_policy: s.run.execution.elicitation_policy,
                        analyst: true,
                        broker: broker.clone(),
                        run_id: id.clone(),
                        prompt_id: s.run.execution.turns.last().unwrap().prompt_id.clone(),
                        session: None,
                    }),
                    agent: AgentId::Codex,
                    executable: std::env::current_exe()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    args: vec![
                        "--exact".into(),
                        "process::interactive::tests::duplex_fixture".into(),
                        "--ignored".into(),
                        "--nocapture".into(),
                    ],
                    cwd: directory.path().into(),
                    env: HashMap::from([
                        ("DUPLEX_AGENT".into(), "codex".into()),
                        ("DUPLEX_MODE".into(), "elicitation".into()),
                        ("MCP_MODE".into(), "confirmation".into()),
                        ("MCP_ACTION".into(), action.into()),
                    ]),
                    input: Some("测试".into()),
                    deadline: Duration::from_secs(5),
                },
            )
            .await
            .unwrap();
            for task in replies.into_inner().unwrap() {
                task.await.unwrap();
            }
            let reopened = Store::open(directory.path()).unwrap();
            let saved = reopened.snapshot(&id).unwrap();
            assert_eq!(saved.run.execution.turns.len(), 1);
            assert!(!saved.run.execution.chain_active);
            assert_eq!(saved.run.execution.elicitation_policy, policy);
            let turn = &saved.run.execution.turns[0];
            assert_eq!(turn.interactions.len(), 1);
            assert_eq!(turn.interactions[0].decision.as_deref(), Some(action));
            assert_eq!(turn.interactions[0].delivery.as_deref(), Some("delivered"));
            assert_eq!(turn.interactions[0].protocol_id.as_deref(), Some("0"));
            assert_eq!(turn.interactions[0].policy, policy);
            if action == "accept" {
                assert_eq!(saved.run.state, RunState::Ready);
            } else {
                assert_eq!(saved.run.state, RunState::Paused);
                assert_eq!(turn.status, TurnStatus::Cancelled);
                assert!(saved.checkpoint.next_step.contains("自动交接已停止"));
                reopened.acknowledge_recovery(&id).unwrap();
                assert_eq!(reopened.snapshot(&id).unwrap().run.execution.turns.len(), 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventKind, RunState};
    use std::io::{Read, Write};

    #[test]
    #[ignore = "Subprocess fixture, invoked by worker tests"]
    fn cli_fixture() {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap();
        eprintln!("{input}");
        let mode = std::env::var("BINWEAVE_WORKER_FIXTURE").unwrap();
        if mode == "stall_session" {
            let session = std::env::var("BINWEAVE_SESSION").unwrap();
            let value = if std::env::var("BINWEAVE_AGENT").unwrap() == "codex" {
                serde_json::json!({"type":"thread.started","thread_id":session})
            } else {
                serde_json::json!({"type":"system","subtype":"init","session_id":session})
            };
            println!("{value}");
            std::io::stdout().flush().unwrap();
            std::thread::sleep(Duration::from_secs(20));
            return;
        }
        if mode == "chain" {
            let text = std::env::var("BINWEAVE_REPLY").unwrap();
            if std::env::var("BINWEAVE_AGENT").unwrap() == "codex" {
                println!(
                    "{}",
                    serde_json::json!({"type":"item.completed","item":{"type":"agent_message","text":text}})
                );
                println!("{}", r#"{"type":"turn.completed"}"#);
            } else {
                println!(
                    "{}",
                    serde_json::json!({"type":"result","is_error":false,"result":text})
                );
            }
            return;
        }
        if mode == "codex" {
            println!(
                "{}",
                r#"{"type":"item.completed","item":{"type":"agent_message","text":"Codex 原文"}}"#
            );
            println!("{}", r#"{"type":"turn.completed"}"#);
        } else if mode == "claude" {
            println!(
                "{}",
                r#"{"type":"result","is_error":false,"result":"Claude 原文"}"#
            );
        } else if mode == "failure" {
            println!("{}", r#"{"type":"turn.failed"}"#);
        }
        std::io::stdout().flush().unwrap();
    }

    #[tokio::test]
    async fn executes_both_adapters_and_does_not_treat_exit_zero_as_completion() {
        for (agent, mode, success) in [
            (AgentId::Codex, "codex", true),
            (AgentId::Claude, "claude", true),
            (AgentId::Codex, "failure", false),
            (AgentId::Codex, "incomplete", false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(Store::open(dir.path()).unwrap());
            let initial = store
                .ensure_active_run(&dir.path().to_string_lossy())
                .unwrap();
            let prompt = "原文 \" ; $(不会执行)\n第二行";
            store
                .begin_prompt(&initial.run.id, agent, prompt, None)
                .unwrap();
            let mut config = spec(agent, &initial.run.workspace, prompt);
            assert_eq!(
                config.executable,
                if agent == AgentId::Codex {
                    "codex"
                } else {
                    "claude"
                }
            );
            assert!(!config
                .args
                .iter()
                .any(|arg| arg.contains(prompt) || arg.contains("bypass")));
            config.executable = std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            config.args = vec![
                "--exact".into(),
                "worker::tests::cli_fixture".into(),
                "--ignored".into(),
                "--nocapture".into(),
            ];
            config
                .env
                .insert("BINWEAVE_WORKER_FIXTURE".into(), mode.into());
            config.deadline = Duration::from_secs(5);
            let (_sender, receiver) = oneshot::channel();
            execute(
                Arc::clone(&store),
                &initial.run.id,
                config,
                receiver,
                |_| {},
            )
            .await
            .unwrap();
            let saved = store.snapshot(&initial.run.id).unwrap();
            assert_eq!(
                saved.run.state,
                if success {
                    RunState::Ready
                } else {
                    RunState::Paused
                }
            );
            assert!(saved.events.iter().any(|event| event.agent == Some(agent)
                && event.kind == EventKind::Stderr
                && event.content.contains("不会执行")));
            if success {
                assert!(saved.events.iter().any(|event| event.agent == Some(agent)
                    && event.kind == EventKind::Message
                    && event.content.contains("原文")));
            }
        }
    }

    #[tokio::test]
    async fn launch_failure_is_durable_and_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let initial = store
            .ensure_active_run(&dir.path().to_string_lossy())
            .unwrap();
        store
            .begin_prompt(&initial.run.id, AgentId::Claude, "test", None)
            .unwrap();
        let mut config = spec(AgentId::Claude, &initial.run.workspace, "test");
        config.executable = "binweave-nonexistent-cli-123".into();
        let (_sender, receiver) = oneshot::channel();
        execute(
            Arc::clone(&store),
            &initial.run.id,
            config,
            receiver,
            |_| {},
        )
        .await
        .unwrap();
        let saved = store.snapshot(&initial.run.id).unwrap();
        assert_eq!(saved.run.state, RunState::Paused);
        assert!(saved.events.last().unwrap().content.contains("失败"));
    }
}

pub fn spec(agent: AgentId, workspace: &str, content: &str) -> RunSpec {
    let (executable, args) = match agent {
        AgentId::Codex => (
            "codex",
            vec![
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--sandbox",
                "workspace-write",
                "-",
            ],
        ),
        AgentId::Claude => (
            "claude",
            vec![
                "--print",
                "--verbose",
                "--output-format",
                "stream-json",
                "--permission-mode",
                "dontAsk",
            ],
        ),
    };
    RunSpec {
        interactive: None,
        agent,
        executable: executable.into(),
        args: args.into_iter().map(str::to_owned).collect(),
        cwd: PathBuf::from(workspace),
        env: HashMap::new(),
        input: Some(content.into()),
        deadline: Duration::from_secs(1800),
    }
}

pub fn turn_spec(
    snapshot: &RunSnapshot,
    agent: AgentId,
    content: &str,
    automatic: bool,
) -> RunSpec {
    let turn_id = &snapshot
        .run
        .execution
        .turns
        .last()
        .expect("accepted turn")
        .prompt_id;
    let evidence: Vec<_> = snapshot
        .events
        .iter()
        .rev()
        .filter(|e| {
            matches!(
                e.kind,
                crate::model::EventKind::Message
                    | crate::model::EventKind::Prompt
                    | crate::model::EventKind::ToolSummary
            )
        })
        .take(16)
        .map(|e| format!("#{}: {}", e.sequence, e.id))
        .collect();
    let input = format!("{}\n\n以下本轮调度契约优先于历史会话中的交接约定：\n{}\n可引用的已保存事件标识（不确定时 evidenceIds 留空，将证据写入 instruction）：\n{}\n\n用户或对方智能体的指令（保持原文）：\n{}", crate::orchestrator::skill_contract(&snapshot.run.roles), crate::handoff::contract(agent, turn_id, automatic), evidence.join("\n"), content);
    let input = format!(
        "{}\n\n{}",
        crate::orchestrator::task_skill_context(snapshot, agent, automatic),
        input
    );
    let role = snapshot.run.roles.role(agent);
    let skill = snapshot
        .run
        .execution
        .turns
        .last()
        .and_then(|t| t.role_skill.as_ref());
    let input = format!("当前 logical_role={}，provider={agent:?}，角色版本={}。\n角色技能来源：{}\n<role-skill>\n{}\n</role-skill>\n\n{}\n\n{}", role.name(), snapshot.run.roles.revision,
        skill.map_or("历史轮次（无角色技能快照）", |s| s.path.as_str()),
        skill.map_or("", |s| s.content.as_str()), input,
        crate::handoff::role_contract(&snapshot.run.roles, agent, turn_id, automatic));
    let mut config = spec(agent, &snapshot.run.workspace, &input);
    if role == crate::model::LogicalRole::Analyst {
        config.input = Some(format!("{}\n\n本轮强制读取边界（优先于历史技能）：\n{}\n当前唯一授权本地根目录：{}。runs/<run_id>/ 仅在该目录内部时属于本轮范围；未配置外部运行目录。不可把沙箱 read-only 误认为任意系统路径授权。",
            config.input.as_deref().unwrap_or(""), include_str!("../../.rule/analyst/SKILL.md"), snapshot.run.workspace));
        if let Some(index) = config.args.iter().position(|a| a == "workspace-write") {
            config.args[index] = "read-only".into();
        }
        if agent == AgentId::Claude {
            config.args.extend([
                "--disallowedTools".into(),
                "Edit,Write,NotebookEdit,Bash".into(),
            ]);
        }
    }
    if let Some(session) = snapshot.run.execution.session(agent) {
        match agent {
            AgentId::Codex => {
                // Sandbox is an exec-level option, before the resume subcommand.
                config.args = [
                    "exec",
                    "--sandbox",
                    if role == crate::model::LogicalRole::Analyst {
                        "read-only"
                    } else {
                        "workspace-write"
                    },
                    "resume",
                    "--json",
                    "--skip-git-repo-check",
                    session,
                    "-",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect();
            }
            AgentId::Claude => {
                config.args.extend(["--resume".into(), session.into()]);
            }
        }
    }
    config
}

/// A single cancellation signal spans the whole chain, including handoff gaps.
#[allow(dead_code)] // Also used by the standalone acceptance probes.
pub async fn execute_chain<F: Fn(RunSnapshot) + Send + Sync>(
    store: Arc<Store>,
    snapshot: RunSnapshot,
    agent: AgentId,
    content: String,
    automatic: bool,
    cancel: tokio::sync::watch::Receiver<bool>,
    publish: F,
) -> Result<(), String> {
    execute_chain_with(
        store, snapshot, agent, content, automatic, cancel, publish, turn_spec,
    )
    .await
}

pub async fn execute_interactive_chain<F: Fn(RunSnapshot) + Send + Sync>(
    store: Arc<Store>,
    snapshot: RunSnapshot,
    agent: AgentId,
    content: String,
    automatic: bool,
    cancel: tokio::sync::watch::Receiver<bool>,
    publish: F,
    broker: Arc<crate::process::interactive::Broker>,
) -> Result<(), String> {
    execute_chain_with(
        store,
        snapshot,
        agent,
        content,
        automatic,
        cancel,
        publish,
        |s, a, c, auto| {
            let mut spec = turn_spec(s, a, c, auto);
            spec.args = match a {
                AgentId::Codex => vec!["app-server".into()],
                AgentId::Claude => {
                    let mut args: Vec<String> = [
                        "--print",
                        "--verbose",
                        "--input-format",
                        "stream-json",
                        "--include-partial-messages",
                        "--output-format",
                        "stream-json",
                        "--permission-mode",
                        "manual",
                        "--permission-prompt-tool",
                        "stdio",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
                    if let Some(session) = s.run.execution.session(a) {
                        args.extend(["--resume".into(), session.into()]);
                    }
                    if s.run.roles.role(a) == crate::model::LogicalRole::Analyst {
                        args.extend([
                            "--restricted".into(),
                            "--tools".into(),
                            "Read,Glob,Grep,AskUserQuestion".into(),
                            "--disallowedTools".into(),
                            "Edit,Write,NotebookEdit,Bash".into(),
                        ]);
                    }
                    args
                }
            };
            spec.interactive = Some(crate::process::interactive::Config {
                elicitation_policy: s.run.execution.elicitation_policy,
                broker: Arc::clone(&broker),
                run_id: s.run.id.clone(),
                prompt_id: s.run.execution.turns.last().unwrap().prompt_id.clone(),
                session: s.run.execution.session(a).map(str::to_owned),
                analyst: s.run.roles.role(a) == crate::model::LogicalRole::Analyst,
            });
            spec
        },
    )
    .await
}

#[cfg(test)]
mod chain_tests {
    use super::*;
    use crate::model::{RunState, TurnStatus};

    #[tokio::test]
    async fn cancelled_workers_can_resume_their_sessions_without_replaying_old_turns() {
        for agent in [AgentId::Codex, AgentId::Claude] {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(Store::open(dir.path()).unwrap());
            let initial = store
                .ensure_active_run(&dir.path().to_string_lossy())
                .unwrap();
            let id = &initial.run.id;
            let session = uuid::Uuid::new_v4().to_string();
            let name = if agent == AgentId::Codex {
                "codex"
            } else {
                "claude"
            };
            let fixture = |s: &RunSnapshot, agent, content: &str, automatic, stall| {
                let mut config = turn_spec(s, agent, content, automatic);
                config.executable = std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                config.args = vec![
                    "--exact".into(),
                    "worker::tests::cli_fixture".into(),
                    "--ignored".into(),
                    "--nocapture".into(),
                ];
                config.env.insert(
                    "BINWEAVE_WORKER_FIXTURE".into(),
                    if stall { "stall_session" } else { name }.into(),
                );
                config.env.insert("BINWEAVE_AGENT".into(), name.into());
                config
                    .env
                    .insert("BINWEAVE_SESSION".into(), session.clone());
                config.deadline = Duration::from_secs(5);
                config
            };
            let sent = store
                .begin_prompt(id, agent, "旧任务不可自动重放", None)
                .unwrap();
            let (stop, receiver) = tokio::sync::watch::channel(false);
            execute_chain_with(
                Arc::clone(&store),
                sent,
                agent,
                "旧任务不可自动重放".into(),
                true,
                receiver,
                |s| {
                    if s.run.execution.session(agent).is_some() {
                        let _ = stop.send(true);
                    }
                    if s.run.state == RunState::Paused && s.run.execution.chain_active {
                        assert!(store.acknowledge_recovery(id).is_err());
                    }
                },
                |s, a, c, auto| fixture(s, a, c, auto, true),
            )
            .await
            .unwrap();
            let stopped = store.snapshot(id).unwrap();
            assert_eq!(stopped.run.state, RunState::Paused);
            assert!(!stopped.run.execution.chain_active);
            assert_eq!(stopped.run.execution.turns[0].status, TurnStatus::Cancelled);
            assert_eq!(stopped.run.execution.session(agent), Some(session.as_str()));
            assert!(store
                .begin_prompt(id, agent, "未确认不能发送", None)
                .is_err());
            let ready = store.acknowledge_recovery(id).unwrap();
            assert_eq!(ready.run.state, RunState::Ready);
            assert_eq!(ready.run.execution.turns.len(), 1);
            assert_eq!(ready.run.execution.session(agent), Some(session.as_str()));
            assert!(store.acknowledge_recovery(id).is_err());
            let next = store
                .begin_prompt(id, agent, "检查现状后继续剩余任务", None)
                .unwrap();
            let config = turn_spec(&next, agent, "检查现状后继续剩余任务", false);
            assert!(config.args.contains(&session));
            assert!(!config.input.unwrap().contains("旧任务不可自动重放"));
            let (_stop, receiver) = tokio::sync::watch::channel(false);
            execute_chain_with(
                Arc::clone(&store),
                next,
                agent,
                "检查现状后继续剩余任务".into(),
                false,
                receiver,
                |_| {},
                |s, a, c, auto| fixture(s, a, c, auto, false),
            )
            .await
            .unwrap();
            let finished = store.snapshot(id).unwrap();
            assert_eq!(finished.run.state, RunState::Ready);
            assert!(!finished.run.execution.chain_active);
            assert_eq!(finished.run.execution.turns.len(), 2);
            assert_eq!(
                finished.run.execution.turns[0].status,
                TurnStatus::Cancelled
            );
            assert_eq!(
                finished.run.execution.turns[1].status,
                TurnStatus::Completed
            );
        }
    }

    #[tokio::test]
    async fn bounded_bidirectional_chain_and_cancellation_gaps() {
        for mode in [
            "normal",
            "off",
            "cancel_gap",
            "cancel_before",
            "limit",
            "duplicate",
            "bad_evidence",
            "failed",
            "skill",
            "skill_off",
            "skill_cancel",
            "degraded",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(Store::open(dir.path()).unwrap());
            let initial = store
                .ensure_active_run(&dir.path().to_string_lossy())
                .unwrap();
            store.swap_roles(&initial.run.id).unwrap();
            let snapshot = store
                .begin_prompt(&initial.run.id, AgentId::Codex, "开始", None)
                .unwrap();
            let (sender, receiver) = tokio::sync::watch::channel(mode == "cancel_before");
            execute_chain_with(
                Arc::clone(&store),
                snapshot,
                AgentId::Codex,
                "开始".into(),
                !matches!(mode, "off" | "skill_off"),
                receiver,
                |s| {
                    if matches!(mode, "cancel_gap" | "skill_cancel")
                        && s.run.execution.turns.last().unwrap().status == TurnStatus::Completed
                        && (mode != "skill_cancel" || s.run.execution.turns.len() == 2)
                    {
                        sender.send(true).unwrap();
                    }
                },
                |s, agent, content, automatic| {
                    let mut config = turn_spec(s, agent, content, automatic);
                    let n = s.run.execution.turns.len();
                    if mode == "skill" && n == 3 {
                        assert!(config.input.as_ref().unwrap().contains("# fixture skill"));
                        assert!(s.run.execution.active_skill_id.is_some());
                    }
                    let reply = if mode == "degraded" && n == 2 {
                        serde_json::json!({"turnId":s.run.execution.turns.last().unwrap().prompt_id,"logical_role":"analyst","handoff_type":"ANALYSIS_DEGRADED","summary":"语义推导","verified_facts":["反汇编存在间接调用"],"findings_refs":["REA fn+4"],"open_uncertainties":["缺少原始指针字节"],"next_instruction_for_implementer":"仅审阅，不实现"}).to_string()
                    } else if matches!(mode, "normal" | "skill") && n == 3 {
                        "任务完成".into()
                    } else {
                        serde_json::to_string(&crate::handoff::Handoff {
                            binweave: "handoff.v1".into(),
                            turn_id: s.run.execution.turns.last().unwrap().prompt_id.clone(),
                            to: if agent == AgentId::Codex {
                                AgentId::Claude
                            } else {
                                AgentId::Codex
                            },
                            instruction: if mode == "duplicate" {
                                "重复任务".into()
                            } else {
                                format!("任务 {n}")
                            },
                            reason: "验证".into(),
                            skill_update: (mode.starts_with("skill")
                                && (n == 2 || mode == "skill_off"))
                                .then(|| crate::handoff::SkillUpdate {
                                    base_artifact_id: None,
                                    content: "# fixture skill\n只执行有证据支持的实现。".into(),
                                }),
                            evidence_ids: if mode == "bad_evidence" {
                                vec!["不存在".into()]
                            } else {
                                vec![]
                            },
                        })
                        .unwrap()
                    };
                    config.executable = std::env::current_exe()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    config.args = vec![
                        "--exact".into(),
                        "worker::tests::cli_fixture".into(),
                        "--ignored".into(),
                        "--nocapture".into(),
                    ];
                    config.env.insert(
                        "BINWEAVE_WORKER_FIXTURE".into(),
                        if mode == "failed" { "failure" } else { "chain" }.into(),
                    );
                    config.env.insert(
                        "BINWEAVE_AGENT".into(),
                        if agent == AgentId::Codex {
                            "codex"
                        } else {
                            "claude"
                        }
                        .into(),
                    );
                    config.env.insert("BINWEAVE_REPLY".into(), reply);
                    config.deadline = Duration::from_secs(10);
                    config
                },
            )
            .await
            .unwrap();
            let saved = store.snapshot(&initial.run.id).unwrap();
            assert!(!saved.run.execution.chain_active, "{mode}");
            let expected = match mode {
                "normal" | "duplicate" | "skill" => 3,
                "skill_cancel" | "degraded" => 2,
                "limit" => 5,
                _ => 1,
            };
            assert_eq!(saved.run.execution.turns.len(), expected, "{mode}");
            let expected_state = if matches!(mode, "normal" | "off" | "skill" | "skill_off") {
                RunState::Ready
            } else {
                RunState::Paused
            };
            assert_eq!(saved.run.state, expected_state, "{mode}");
            if mode.starts_with("skill") {
                assert_eq!(
                    saved.run.execution.active_skill_id.is_some(),
                    mode == "skill"
                );
                assert_eq!(saved.artifacts.len(), if mode == "skill" { 3 } else { 2 });
            }
            if mode == "normal" {
                assert_eq!(
                    saved
                        .run
                        .execution
                        .turns
                        .iter()
                        .map(|t| t.agent)
                        .collect::<Vec<_>>(),
                    vec![AgentId::Codex, AgentId::Claude, AgentId::Codex]
                );
                assert!(saved
                    .run
                    .execution
                    .turns
                    .iter()
                    .all(|t| t.status == TurnStatus::Completed));
            }
            if mode == "cancel_before" {
                assert_eq!(saved.run.execution.turns[0].status, TurnStatus::Cancelled);
            }
        }
    }

    #[test]
    fn resume_arguments_keep_stdin_and_sandbox_and_current_roles() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let initial = store
            .ensure_active_run(&dir.path().to_string_lossy())
            .unwrap();
        let mut s = store
            .begin_prompt(&initial.run.id, AgentId::Codex, "hello", None)
            .unwrap();
        s.run.execution.codex_session = Some(uuid::Uuid::new_v4().to_string());
        s.run.execution.claude_session = Some(uuid::Uuid::new_v4().to_string());
        s.run.roles.analyst = AgentId::Claude;
        s.run.roles.implementer = AgentId::Codex;
        let config = turn_spec(&s, AgentId::Codex, "hello", true);
        assert_eq!(
            &config.args[..4],
            &["exec", "--sandbox", "workspace-write", "resume"]
        );
        assert_eq!(config.args.last().unwrap(), "-");
        assert!(config.input.unwrap().contains("Analyst: Claude Code CLI"));
        let config = turn_spec(&s, AgentId::Claude, "hello", false);
        assert_eq!(config.args[config.args.len() - 2], "--resume");
        assert_eq!(config.args.last(), s.run.execution.claude_session.as_ref());
    }
}

async fn execute_chain_with<F, S>(
    store: Arc<Store>,
    mut snapshot: RunSnapshot,
    mut agent: AgentId,
    mut content: String,
    automatic: bool,
    mut cancel: tokio::sync::watch::Receiver<bool>,
    publish: F,
    make_spec: S,
) -> Result<(), String>
where
    F: Fn(RunSnapshot) + Send + Sync,
    S: Fn(&RunSnapshot, AgentId, &str, bool) -> RunSpec,
{
    let id = snapshot.run.id.clone();
    let mut seen = std::collections::HashSet::new();
    let mut count = 0;
    loop {
        let config = make_spec(&snapshot, agent, &content, automatic);
        let (stop, receiver) = oneshot::channel();
        let mut stop = Some(stop);
        if *cancel.borrow() {
            let _ = stop.take().unwrap().send(());
        }
        let outcome = {
            let work = execute(Arc::clone(&store), &id, config, receiver, &publish);
            tokio::pin!(work);
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.changed(), if stop.is_some() => {
                        let _ = stop.take().unwrap().send(());
                    }
                    result = &mut work => break result?,
                }
            }
        };
        if *cancel.borrow() {
            publish(store.end_chain(
                &id,
                Some("已停止本次执行链，后续交接已取消。请检查输出后确认恢复检查点。"),
                true,
            )?);
            return Ok(());
        }
        let Some((source_id, text)) = outcome.filter(|_| automatic) else {
            publish(store.end_chain(&id, None, false)?);
            return Ok(());
        };
        let request = match crate::handoff::parse_for_roles(
            &text,
            agent,
            &snapshot.run.execution.turns.last().unwrap().prompt_id,
            &snapshot.run.roles,
        ) {
            Ok(Some(request)) => request,
            Ok(None) => {
                publish(store.end_chain(&id, None, false)?);
                return Ok(());
            }
            Err(error) => {
                publish(store.end_chain(&id, Some(&error), false)?);
                return Ok(());
            }
        };
        let key = crate::handoff::dedup_key(&text, &request);
        if crate::handoff::is_degraded(&text) {
            let current = store.snapshot(&id)?;
            let detail = match crate::handoff::validate_answers(&text, &current.run.execution.pending_queries) {
                Ok(()) => format!("{:?} 已回传降级分析：缺失字节风险和语义证据保留在最终回复。自动交接已暂停，待查证问题未清除；请人工补证或审阅后手动移交给 {:?}，不得依据未核实字节继续实现。", agent, request.to),
                Err(error) => error,
            };
            publish(store.end_chain(&id, Some(&detail), false)?);
            return Ok(());
        }
        if count >= crate::handoff::MAX_HANDOFFS || !seen.insert(key) {
            publish(store.end_chain(&id, Some("达到交接次数上限或发现重复任务，已停止派发。最后一条交接请求保留在输出中，请人工检查。"), false)?);
            return Ok(());
        }
        if *cancel.borrow() {
            publish(store.end_chain(&id, Some("交接前收到停止请求，未派发下一轮。"), true)?);
            return Ok(());
        }
        agent = request.to;
        content = request.instruction;
        snapshot = match store.begin_auto_prompt(&id, agent, &content, &source_id) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                publish(store.end_chain(&id, Some(&error), false)?);
                return Ok(());
            }
        };
        count += 1;
        publish(snapshot.clone());
    }
}

pub async fn execute<F: Fn(RunSnapshot) + Send + Sync>(
    store: Arc<Store>,
    id: &str,
    spec: RunSpec,
    cancel: oneshot::Receiver<()>,
    publish: F,
) -> Result<Option<(String, String)>, String> {
    let failed = AtomicBool::new(false);
    let mcp_stopped = AtomicBool::new(false);
    let completed = AtomicBool::new(false);
    let final_reply = Mutex::new(None);
    let error_output = Mutex::new(String::new());
    let result = crate::process::batching::run(&spec, cancel, |outputs| {
        let count = outputs
            .iter()
            .position(|output| {
                output.interaction.is_some() || output.resolved_interaction.is_some()
            })
            .unwrap_or(outputs.len());
        let snapshot = if count > 0 {
            Some(store.append_outputs(id, spec.agent, &outputs[..count])?)
        } else {
            None
        };
        for output in outputs {
            if output.interaction.is_some() || output.resolved_interaction.is_some() {
                if output.interaction.as_ref().is_some_and(|r| {
                    r.kind.starts_with("mcp_")
                        && matches!(r.decision.as_deref(), Some("decline" | "cancel"))
                }) {
                    mcp_stopped.store(true, Ordering::Relaxed);
                }
                if let Some(snapshot) = snapshot {
                    publish(snapshot);
                }
                let config = spec.interactive.as_ref().ok_or("缺少交互通道")?;
                publish(store.interaction(
                    id,
                    &config.prompt_id,
                    output.interaction,
                    output.resolved_interaction.as_deref(),
                )?);
                return Ok(());
            }
            if output.reported_failure {
                failed.store(true, Ordering::Relaxed);
                *error_output.lock().map_err(|_| "错误记录锁异常")? = output.raw.clone();
            } else if output.kind == crate::model::EventKind::Stderr {
                let mut errors = error_output.lock().map_err(|_| "错误记录锁异常")?;
                if errors.len() < 16000 {
                    errors.push_str(&output.raw);
                    errors.push('\n');
                }
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&output.raw) {
                let terminal = match spec.agent {
                    AgentId::Codex => value["type"] == "turn.completed",
                    AgentId::Claude => value["type"] == "result" && value["is_error"] == false,
                };
                if terminal && output.kind != crate::model::EventKind::Stderr {
                    completed.store(true, Ordering::Relaxed);
                }
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&output.raw) {
                let final_message = match spec.agent {
                    AgentId::Codex => {
                        value["type"] == "item.completed"
                            && value["item"]["type"] == "agent_message"
                            && value["item"]["phase"].as_str() != Some("commentary")
                    }
                    AgentId::Claude => value["type"] == "result" && value["is_error"] == false,
                };
                if final_message && output.kind == crate::model::EventKind::Message {
                    if let Some(text) = &output.summary {
                        *final_reply.lock().map_err(|_| "最终回复锁异常")? = Some((
                            snapshot
                                .as_ref()
                                .ok_or("缺少输出记录")?
                                .events
                                .iter()
                                .rev()
                                .find(|event| {
                                    event.agent == Some(spec.agent)
                                        && event.kind == crate::model::EventKind::Message
                                        && event.content == *text
                                })
                                .ok_or("最终回复缺少对应消息记录")?
                                .id
                                .clone(),
                            text.clone(),
                        ));
                    }
                }
            }
        }
        if let Some(snapshot) = snapshot {
            publish(snapshot);
        }
        Ok(())
    })
    .await;
    let success =
        result == Ok(0) && completed.load(Ordering::Relaxed) && !failed.load(Ordering::Relaxed);
    let detail = match result {
        Ok(0) if success => {
            "本轮执行已完成；正在检查后续交接。已捕获的会话标识将用于接续。".to_owned()
        }
        Ok(code) => format!(
            "本轮未正常完成（退出码 {code}；错误代码 {}）。\n{}",
            if failed.load(Ordering::Relaxed) {
                "AGENT_ERROR"
            } else if code == 0 {
                "MISSING_COMPLETION"
            } else {
                "PROCESS_EXIT"
            },
            error_output.into_inner().unwrap_or_default()
        ),
        Err(RunError::Cancelled) => {
            "本轮已停止，请检查可能已产生的文件修改，然后确认恢复检查点。".into()
        }
        Err(RunError::TimedOut) => format!(
            "错误代码 TIMEOUT：本轮超过 {} 秒，已停止，请检查输出和恢复检查点。",
            spec.deadline.as_secs()
        ),
        Err(RunError::ThinkingTimedOut) => "错误代码 THINKING_TIMEOUT：连续思考或等待有效输出已达 5 分钟，已终止 Agent 及其子进程。请检查已有输出和文件修改后确认恢复检查点。".into(),
        Err(RunError::Failed(error)) => format!("工作进程失败；错误代码 WORKER_ERROR：{error}"),
    };
    let success = success && !mcp_stopped.load(Ordering::Relaxed);
    let detail = if mcp_stopped.load(Ordering::Relaxed) {
        format!("MCP 请求已拒绝或取消，自动交接已停止。请检查输出后确认恢复检查点；不会重放旧回答。\n{detail}")
    } else {
        detail
    };
    publish(store.finish_prompt(id, spec.agent, success, &detail)?);
    Ok(if success {
        final_reply.into_inner().map_err(|_| "最终回复锁异常")?
    } else {
        None
    })
}
