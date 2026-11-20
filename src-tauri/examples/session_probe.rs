//! Opt-in real model calls in an isolated directory; never runs in `verify`.
use binweave_lib::{handoff, model, orchestrator, process, storage};
#[path = "../src/worker.rs"]
#[allow(dead_code)]
mod worker;
use model::AgentId;
use std::{path::PathBuf, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), String> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("请提供隔离检查的父目录绝对路径")?,
    );
    if !root.is_absolute() {
        return Err("必须使用绝对路径".into());
    }
    let directory = root.join(format!("session-probe-{}", uuid::Uuid::new_v4()));
    let workspace = directory.join("workspace");
    std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
    let store = Arc::new(storage::Store::open(&directory.join("records"))?);
    let initial = store.ensure_active_run(&workspace.to_string_lossy())?;
    println!("隔离工作区及完整记录：{}", directory.display());
    let mut failures = 0;
    for agent in [AgentId::Codex, AgentId::Claude] {
        let marker = format!("probe-{}", uuid::Uuid::new_v4());
        for round in 0..2 {
            let prompt = if round == 0 {
                format!("这是会话接续检查。不要调用任何工具、读取文件或修改文件。记住标记 {marker}，只回复这个标记。")
            } else {
                "这是同一会话的第二轮。不要调用任何工具、读取文件或修改文件。只回复上一轮要求记住的标记。".into()
            };
            let snapshot = store.begin_prompt(&initial.run.id, agent, &prompt, None)?;
            let mut spec = worker::turn_spec(&snapshot, agent, &prompt, false);
            spec.deadline = Duration::from_secs(75);
            let (_sender, receiver) = tokio::sync::oneshot::channel();
            let result =
                worker::execute(Arc::clone(&store), &initial.run.id, spec, receiver, |_| {})
                    .await?;
            store.end_chain(&initial.run.id, None, false)?;
            let saved = store.snapshot(&initial.run.id)?;
            let matches = result
                .as_ref()
                .is_some_and(|(_, text)| text.contains(&marker));
            println!(
                "{agent:?} 第 {} 轮：状态 {:?}，会话已保存 {}，标记匹配 {matches}",
                round + 1,
                saved.run.state,
                saved.run.execution.session(agent).is_some()
            );
            if saved.run.state != model::RunState::Ready
                || !matches
                || saved.run.execution.session(agent).is_none()
            {
                failures += 1;
                for event in saved
                    .events
                    .iter()
                    .rev()
                    .filter(|e| e.agent == Some(agent))
                    .take(4)
                {
                    println!("{:?}: {}", event.kind, event.content);
                }
                if saved.run.state != model::RunState::Ready {
                    store.transition(
                        &initial.run.id,
                        model::RunState::Ready,
                        "隔离检查记录错误，不重试本轮，继续检查另一 CLI。",
                    )?;
                }
                break;
            }
        }
    }
    if failures > 0 {
        return Err(format!(
            "{failures} 个智能体未通过完整会话接续检查；请查看隔离记录。"
        ));
    }
    Ok(())
}
