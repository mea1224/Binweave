//! Real CLI cancellation and invalid-resume checks, opt-in and isolated.
use binweave_lib::{handoff, model, orchestrator, process, storage};
#[path = "../src/worker.rs"]
#[allow(dead_code)]
mod worker;
use model::{AgentId, EdgeType, RunSnapshot, RunState, TurnStatus};
use std::{path::PathBuf, sync::Arc, time::Duration};

async fn bounded<F: Fn(RunSnapshot) + Send + Sync>(
    store: Arc<storage::Store>,
    snapshot: RunSnapshot,
    agent: AgentId,
    prompt: String,
    sender: &tokio::sync::watch::Sender<bool>,
    publish: F,
) -> Result<(), String> {
    let work = worker::execute_chain(
        store,
        snapshot,
        agent,
        prompt,
        true,
        sender.subscribe(),
        publish,
    );
    tokio::pin!(work);
    tokio::select! {
        result = &mut work => result,
        _ = tokio::time::sleep(Duration::from_secs(120)) => {
            let _ = sender.send(true);
            work.await?;
            Err("检查超时，已等待工作进程清理".into())
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let filter = std::env::args().nth(2);
    if filter
        .as_deref()
        .is_some_and(|filter| filter != "claude-invalid-resume")
    {
        return Err("可选检查项仅支持 claude-invalid-resume".into());
    }
    let root = PathBuf::from(std::env::args().nth(1).ok_or("请提供绝对父目录")?);
    if !root.is_absolute() {
        return Err("必须使用绝对路径".into());
    }
    let directory = root.join(format!("control-probe-{}", uuid::Uuid::new_v4()));
    println!("隔离验收目录：{}", directory.display());
    for agent in [AgentId::Codex, AgentId::Claude] {
        for mode in ["running", "gap", "invalid_resume"] {
            if filter.is_some() && (agent != AgentId::Claude || mode != "invalid_resume") {
                continue;
            }
            let case = directory.join(format!("{agent:?}-{mode}"));
            let workspace = case.join("workspace");
            std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
            let records = case.join("records");
            let store = Arc::new(storage::Store::open(&records)?);
            let initial = store.ensure_active_run(&workspace.to_string_lossy())?;
            if agent == AgentId::Claude {
                store.swap_roles(&initial.run.id)?;
            }
            let initial_edges = store.snapshot(&initial.run.id)?.edges.len();
            let prompt = if mode == "gap" {
                "请分析 JavaScript 的 function clamp(v,min,max){return Math.min(min,Math.max(max,v));} 的错误，并用本轮 handoff.v1 把具体修复要求交给对方实现员。输入限于有限数值，min <= max。不要使用工具或读写文件。".to_owned()
            } else {
                "请说明稳定排序的定义，不使用工具或读写文件。".to_owned()
            };
            let mut accepted = store.begin_prompt(&initial.run.id, agent, &prompt, None)?;
            if mode == "invalid_resume" {
                // Deliberate negative fixture: valid UUID, but never created by either CLI.
                // This checks CLI resume failure, not authentication or a fabricated provider outage.
                accepted = store.append_output(
                    &initial.run.id,
                    agent,
                    &process::parser::ParsedOutput {
                        interaction: None,
                        resolved_interaction: None,
                        kind: model::EventKind::System,
                        raw: "隔离故障注入：不存在的 CLI 会话 UUID".into(),
                        summary: None,
                        session_id: Some(uuid::Uuid::new_v4().to_string()),
                        reported_failure: false,
                    },
                )?;
            }
            let (sender, _) = tokio::sync::watch::channel(false);
            bounded(
                Arc::clone(&store),
                accepted,
                agent,
                prompt,
                &sender,
                |snapshot| {
                    let status = snapshot.run.execution.turns.last().unwrap().status;
                    if (mode == "running" && status == TurnStatus::Running)
                        || (mode == "gap" && status == TurnStatus::Completed)
                    {
                        let _ = sender.send(true);
                    }
                },
            )
            .await?;
            let saved = store.snapshot(&initial.run.id)?;
            std::fs::write(
                case.join("snapshot.json"),
                serde_json::to_vec_pretty(&saved).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let expected = match mode {
                "running" => TurnStatus::Cancelled,
                "gap" => TurnStatus::Completed,
                _ => TurnStatus::Failed,
            };
            if saved.run.execution.chain_active
                || saved.run.execution.turns.len() != 1
                || saved.run.execution.turns[0].status != expected
                || saved.run.state != RunState::Paused
                || saved
                    .edges
                    .iter()
                    .skip(initial_edges)
                    .any(|edge| edge.edge_type == EdgeType::Handoff)
            {
                return Err(format!("{agent:?} {mode} 状态断言失败，请检查快照"));
            }
            if mode == "gap"
                && !saved
                    .events
                    .iter()
                    .filter(|event| {
                        event.agent == Some(agent) && event.kind == model::EventKind::Message
                    })
                    .any(|event| {
                        handoff::parse(
                            &event.content,
                            agent,
                            &saved.run.execution.turns[0].prompt_id,
                        )
                        .is_ok_and(|request| request.is_some())
                    })
            {
                return Err("未产生真实有效交接请求，不能将本轮计作交接间隙停止通过".into());
            }
            drop(store);
            let restarted = storage::Store::open(&records)?;
            restarted.recover_interrupted()?;
            let recovered = restarted.ensure_active_run(&workspace.to_string_lossy())?;
            if recovered.run.execution.turns.len() != 1
                || recovered.events.len() != saved.events.len()
                || recovered.run.execution.session(agent) != saved.run.execution.session(agent)
            {
                return Err("重开存储后记录或会话发生非预期变化".into());
            }
            let acknowledged = restarted.transition(
                &initial.run.id,
                RunState::Ready,
                "隔离验收：已检查故障/停止记录，不重放",
            )?;
            if acknowledged.run.execution.turns.len() != 1
                || acknowledged.run.execution.chain_active
            {
                return Err("确认检查点意外产生新轮次".into());
            }
            println!("通过 {agent:?} {mode}：单轮 {expected:?}、暂停、无后续派发；重开存储并确认后无重放。");
        }
    }
    Ok(())
}
