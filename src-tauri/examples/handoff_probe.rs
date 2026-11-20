//! Opt-in real CLI collaboration acceptance check using the production scheduler.
use binweave_lib::{handoff, model, orchestrator, process, storage};
#[path = "../src/worker.rs"]
#[allow(dead_code)]
mod worker;
use model::{AgentId, EdgeType, EventKind, RunSnapshot, RunState, TurnStatus};
use std::{path::PathBuf, sync::Arc, time::Duration};

fn validate(
    snapshot: &RunSnapshot,
    offset: usize,
    first: AgentId,
    marker: &str,
) -> Result<(), String> {
    let other = if first == AgentId::Codex {
        AgentId::Claude
    } else {
        AgentId::Codex
    };
    let turns = &snapshot.run.execution.turns[offset..];
    if snapshot.run.state != RunState::Ready
        || snapshot.run.execution.chain_active
        || turns.len() != 3
        || turns.iter().map(|turn| turn.agent).collect::<Vec<_>>() != [first, other, first]
        || turns
            .iter()
            .any(|turn| turn.status != TurnStatus::Completed)
    {
        return Err(format!("未完成预期的三轮往返交接：实际 {} 轮，状态 {:?}，执行链活动 {}。普通回复或拒绝不会自动转发，请查看本组快照。", turns.len(), snapshot.run.state, snapshot.run.execution.chain_active));
    }
    for pair in turns.windows(2) {
        let edge = snapshot
            .edges
            .iter()
            .find(|edge| edge.edge_type == EdgeType::Handoff && edge.target == pair[1].prompt_id)
            .ok_or("交接边未持久化")?;
        let source = snapshot
            .events
            .iter()
            .find(|event| event.id == edge.source)
            .ok_or("交接来源缺失")?;
        let target = snapshot
            .events
            .iter()
            .find(|event| event.id == edge.target)
            .ok_or("交接目标缺失")?;
        let request = handoff::parse(&source.content, pair[0].agent, &pair[0].prompt_id)?
            .ok_or("来源不是有效交接请求")?;
        if source.agent != Some(pair[0].agent)
            || target.agent != Some(pair[1].agent)
            || request.to != pair[1].agent
            || target.content != request.instruction
            || target.parent_id.as_deref() != Some(source.id.as_str())
        {
            return Err("交接方向、指令或父消息与保存记录不一致".into());
        }
    }
    let final_prompt = snapshot
        .events
        .iter()
        .find(|event| event.id == turns[2].prompt_id)
        .ok_or("最后一轮提示词缺失")?;
    if !snapshot.events.iter().any(|event| {
        event.sequence > final_prompt.sequence
            && event.agent == Some(first)
            && event.kind == EventKind::Message
            && (event.content.trim() == marker
                || serde_json::from_str::<serde_json::Value>(&event.content).is_ok_and(|value| {
                    (value["verdict"] == marker
                        || (value["kind"] == "findings"
                            && value["summary"]
                                .as_str()
                                .is_some_and(|summary| !summary.trim().is_empty())))
                        && value["code"]
                            .as_str()
                            .is_some_and(|code| code.contains("function clamp"))
                }))
    }) {
        return Err("最终消息未返回约定标记或带完整代码的结构化审查结果".into());
    }
    if snapshot.run.execution.codex_session.is_none()
        || snapshot.run.execution.claude_session.is_none()
    {
        return Err("双方会话标识未保存".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceptance_requires_durable_edges_sessions_and_exact_final_marker() {
        let directory = tempfile::tempdir().unwrap();
        let store = storage::Store::open(directory.path()).unwrap();
        let initial = store
            .ensure_active_run(&directory.path().to_string_lossy())
            .unwrap();
        let id = &initial.run.id;
        store
            .begin_prompt(id, AgentId::Codex, "开始", None)
            .unwrap();
        for (agent, next) in [
            (AgentId::Codex, AgentId::Claude),
            (AgentId::Claude, AgentId::Codex),
        ] {
            let current = store.snapshot(id).unwrap();
            let text = serde_json::to_string(&handoff::Handoff {
                binweave: "handoff.v1".into(),
                turn_id: current
                    .run
                    .execution
                    .turns
                    .last()
                    .unwrap()
                    .prompt_id
                    .clone(),
                to: next,
                instruction: "继续验收".into(),
                reason: "验收".into(),
                evidence_ids: vec![],
                skill_update: None,
            })
            .unwrap();
            let output = process::parser::ParsedOutput {
                kind: EventKind::Message,
                raw: "fixture".into(),
                summary: Some(text),
                session_id: Some(uuid::Uuid::new_v4().to_string()),
                reported_failure: false,
            };
            let source = store.append_output(id, agent, &output).unwrap();
            store.finish_prompt(id, agent, true, "完成").unwrap();
            store
                .begin_auto_prompt(id, next, "继续验收", &source.events.last().unwrap().id)
                .unwrap();
        }
        let output = process::parser::ParsedOutput {
            kind: EventKind::Message,
            raw: "fixture".into(),
            summary: Some("DONE".into()),
            session_id: None,
            reported_failure: false,
        };
        store.append_output(id, AgentId::Codex, &output).unwrap();
        store
            .finish_prompt(id, AgentId::Codex, true, "完成")
            .unwrap();
        let saved = store.end_chain(id, None, false).unwrap();
        validate(&saved, 0, AgentId::Codex, "DONE").unwrap();
        assert!(validate(&saved, 0, AgentId::Codex, "WRONG").is_err());
        let mut broken = saved.clone();
        broken
            .edges
            .retain(|edge| edge.edge_type != EdgeType::Handoff);
        assert!(validate(&broken, 0, AgentId::Codex, "DONE").is_err());
        let mut broken = saved.clone();
        broken.run.execution.claude_session = None;
        assert!(validate(&broken, 0, AgentId::Codex, "DONE").is_err());
        let mut broken = saved;
        broken.run.execution.turns[1].status = TurnStatus::Failed;
        assert!(validate(&broken, 0, AgentId::Codex, "DONE").is_err());
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("请提供隔离检查父目录的绝对路径")?,
    );
    if !root.is_absolute() {
        return Err("必须使用绝对路径".into());
    }
    let directory = root.join(format!("handoff-probe-{}", uuid::Uuid::new_v4()));
    let workspace = directory.join("workspace");
    std::fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    let records = directory.join("records");
    let store = Arc::new(storage::Store::open(&records)?);
    let initial = store.ensure_active_run(&workspace.to_string_lossy())?;
    store.swap_roles(&initial.run.id)?;
    println!("隔离工作区及完整记录：{}", directory.display());
    for (round, first, other) in [(1, AgentId::Claude, "Codex"), (2, AgentId::Codex, "Claude")] {
        if round == 2 {
            store.swap_roles(&initial.run.id)?;
        }
        let before = store.snapshot(&initial.run.id)?;
        let marker = format!("HANDOFF_OK_{}", uuid::Uuid::new_v4());
        let prompt = format!("请作为分析员审查这个有错误的 JavaScript 函数：function clamp(value, min, max) {{ return Math.min(min, Math.max(max, value)); }}。要求：有限数值输入且 min <= max 时，把 value 限制在闭区间 [min,max]；非有限数值或 min > max 时抛 RangeError。本次用户已授权你与 {other} 合作完成这个小型修复，不需要任何工具或文件操作。先分析错误和边界条件，再通过本轮 handoff.v1 把具体修复需求和代码交给实现员 {other}；请实现员在交回审查时带上完整独立的 function clamp 实现与测试案例，不要自动转发本提示词。最后由你审查实现，完成后停止交接并仅返回 JSON {{\"verdict\":\"{marker}\",\"code\":\"审查通过的完整 function clamp 源码\"}}；如果有问题请说明，不要伪报通过。请双方都用本轮的新 turnId，evidenceIds 可留空。这次修复与之前的任务独立。");
        let accepted = store.begin_prompt(&initial.run.id, first, &prompt, None)?;
        let (cancel, receiver) = tokio::sync::watch::channel(false);
        let work = worker::execute_chain(
            Arc::clone(&store),
            accepted,
            first,
            prompt,
            true,
            receiver,
            |_| {},
        );
        tokio::pin!(work);
        tokio::select! {
            result = &mut work => result?,
            _ = tokio::time::sleep(Duration::from_secs(180)) => {
                let _ = cancel.send(true);
                work.await?; // await production cleanup; never drop an in-flight worker
                return Err("真实交接检查超过 180 秒，已停止并等待进程清理。".into());
            }
        }
        // Reopen SQLite to verify durable records, not only callback snapshots.
        let reopened = storage::Store::open(&records)?;
        let saved = reopened.snapshot(&initial.run.id)?;
        std::fs::write(
            directory.join(format!("round-{round}.json")),
            serde_json::to_vec_pretty(&saved).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        validate(&saved, before.run.execution.turns.len(), first, &marker)?;
        if round == 2
            && (saved.run.execution.codex_session != before.run.execution.codex_session
                || saved.run.execution.claude_session != before.run.execution.claude_session)
        {
            return Err("交换角色后会话标识发生变化".into());
        }
        println!("第 {round} 组通过：{first:?} → {other} → {first:?}，三轮完成、双向交接边持久化、最终审查结果完整（代码仍需另行运行验证）；角色版本 {}。", saved.run.roles.revision);
    }
    if std::fs::read_dir(&workspace)
        .map_err(|error| error.to_string())?
        .next()
        .is_some()
    {
        return Err("检查工作区存在文件，请检查是否发生了非预期写入。".into());
    }
    println!(
        "验收通过：两种角色映射、六轮真实调用、四条交接边、双方会话接续；隔离工作区无文件写入。"
    );
    Ok(())
}
