use crate::{
    mcp_probe,
    model::{AgentId, Bootstrap, EnvCheckReport, RunSnapshot, RunState},
    storage::Store,
    AppState,
};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(job)
        .await
        .map_err(|_| "Background operation failed".to_owned())?
}

#[tauri::command]
pub async fn submit_prompt(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: String,
    agent: AgentId,
    content: String,
    source_id: Option<String>,
    auto_handoff: Option<bool>,
    requirements: Option<String>,
    continuation_id: Option<String>,
    restart_id: Option<String>,
) -> Result<RunSnapshot, String> {
    let workers = Arc::clone(&state.workers);
    let mut slot = workers
        .0
        .try_lock()
        .map_err(|_| "当前正在提交，请勿重复发送")?;
    if slot.is_some() {
        return Err("已有工作进程正在执行，请等待或停止当前执行".into());
    }
    let store = Arc::clone(&state.store);
    let auto_handoff =
        restart_id.is_none() && continuation_id.is_none() && auto_handoff.unwrap_or(false);
    let snapshot = if let Some(prompt_id) = restart_id {
        if continuation_id.is_some() || source_id.is_some() || requirements.is_some() {
            return Err("单侧重试不能同时继续或移交其他任务".into());
        }
        store.restart_turn(&run_id, agent, &prompt_id)?
    } else if let Some(task_id) = continuation_id {
        if source_id.is_some() || requirements.is_some() {
            return Err("继续执行不能同时移交任务".into());
        }
        store.begin_continuation(&run_id, agent, &content, &task_id)?
    } else {
        store.begin_prompt_with_requirements(
            &run_id,
            agent,
            &content,
            source_id.as_deref(),
            requirements.as_deref(),
        )?
    };
    let content = snapshot
        .events
        .last()
        .ok_or("缺少已保存的提示词")?
        .content
        .clone();
    let (sender, receiver) = tokio::sync::watch::channel(false);
    *slot = Some((run_id.clone(), sender));
    drop(slot);
    notify(&app, snapshot.clone());
    let accepted = snapshot.clone();
    tauri::async_runtime::spawn(async move {
        let cursor = std::sync::Mutex::new((
            accepted.events.len(),
            accepted.edges.len(),
            accepted.artifacts.len(),
        ));
        if let Err(error) = crate::worker::execute_interactive_chain(
            Arc::clone(&store),
            accepted,
            agent,
            content,
            auto_handoff,
            receiver,
            |s| {
                // Publish terminal state only after the worker registry is released.
                if s.run.execution.chain_active {
                    let mut cursor = cursor.lock().unwrap();
                    let next = (s.events.len(), s.edges.len(), s.artifacts.len());
                    // Only committed additions cross the IPC boundary during streaming.
                    // Full command responses and the terminal snapshot reconcile missed events.
                    let update = serde_json::json!({
                        "run": s.run, "checkpoint": s.checkpoint,
                        "baseEventCount": cursor.0,
                        "events": &s.events[cursor.0..],
                        "edges": &s.edges[cursor.1..],
                        "artifacts": &s.artifacts[cursor.2..],
                    });
                    let _ = app.emit("run-update", update);
                    *cursor = next;
                }
            },
            Arc::clone(&workers.1),
        )
        .await
        {
            let _ = app.emit(
                "worker-error",
                format!("执行结果保存失败，请刷新并检查恢复状态：{error}"),
            );
            let _ = store.recover_interrupted();
        }
        let mut slot = workers.0.lock().await;
        slot.take();
        if let Ok(snapshot) = store.snapshot(&run_id) {
            notify(&app, snapshot);
        }
    });
    Ok(snapshot)
}

#[tauri::command]
pub async fn respond_interaction(
    state: State<'_, AppState>,
    run_id: String,
    prompt_id: String,
    request_id: String,
    answer: serde_json::Value,
) -> Result<RunSnapshot, String> {
    {
        let guard = state.workers.0.lock().await;
        if !guard
            .as_ref()
            .is_some_and(|(id, stop)| id == &run_id && !*stop.borrow())
        {
            return Err("当前任务已停止，交互不可用".into());
        }
    }
    state
        .workers
        .1
        .respond(&run_id, &prompt_id, &request_id, answer)
        .await?;
    state.store.snapshot(&run_id)
}

#[tauri::command]
pub async fn set_elicitation_policy(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: String,
    policy: crate::model::ElicitationPolicy,
) -> Result<RunSnapshot, String> {
    let slot = state.workers.0.lock().await;
    if slot.is_some() {
        return Err("请先停止执行再修改 MCP 交互策略".into());
    }
    let snapshot = state.store.set_elicitation_policy(&run_id, policy)?;
    notify(&app, snapshot.clone());
    Ok(snapshot)
}

#[tauri::command]
pub async fn cancel_prompt(state: State<'_, AppState>, run_id: String) -> Result<(), String> {
    let mut slot = state.workers.0.lock().await;
    let Some((id, sender)) = slot.as_mut() else {
        return Err("当前没有正在执行的工作进程".into());
    };
    if *id != run_id {
        return Err("工作进程不属于当前任务".into());
    }
    // Retain the registry entry until cleanup and persistence finish.
    let _ = sender.send(true);
    Ok(())
}

#[tauri::command]
pub async fn reset_agent_session(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: String,
    agent: AgentId,
) -> Result<RunSnapshot, String> {
    let _guard = state.workers.0.lock().await;
    if _guard.is_some() {
        return Err("请先停止当前执行链".into());
    }
    Ok(notify(&app, state.store.reset_session(&run_id, agent)?))
}
fn notify(app: &AppHandle, snapshot: RunSnapshot) -> RunSnapshot {
    let _ = app.emit("run-snapshot", &snapshot);
    snapshot
}
#[tauri::command]
pub async fn bootstrap(state: State<'_, AppState>) -> Result<Bootstrap, String> {
    let workspace = state.workspace.to_string_lossy().into_owned();
    let store = Arc::clone(&state.store);
    let catalog = blocking(move || store.workspace_catalog(Some(&workspace))).await?;
    Ok(Bootstrap {
        snapshot: catalog.snapshot,
        workspaces: catalog.workspaces,
        data_directory: state.store.directory.to_string_lossy().into_owned(),
    })
}
#[tauri::command]
pub async fn manage_workspace(
    state: State<'_, AppState>,
    operation: String,
    run_id: Option<String>,
    workspace: Option<String>,
    confirmed: Option<bool>,
) -> Result<crate::model::WorkspaceCatalog, String> {
    let guard = state.workers.0.lock().await;
    if guard.is_some() {
        return Err("请先停止任务并等待进程清理，再切换或修改工作区。".into());
    }
    let store = Arc::clone(&state.store);
    blocking(move || {
        store.manage_workspace(
            &operation,
            run_id.as_deref(),
            workspace.as_deref(),
            confirmed.unwrap_or(false),
        )
    })
    .await
}
#[tauri::command]
pub async fn inspect_environment(
    state: State<'_, AppState>,
    run_id: String,
) -> Result<EnvCheckReport, String> {
    let _guard = state
        .diagnostics
        .try_lock()
        .map_err(|_| "Diagnostics are already running")?;
    let store = Arc::clone(&state.store);
    let snapshot = blocking(move || store.snapshot(&run_id)).await?;
    Ok(mcp_probe::inspect_environment(std::path::Path::new(&snapshot.run.workspace)).await)
}
#[tauri::command]
pub async fn delete_history(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: String,
    confirmed: bool,
) -> Result<RunSnapshot, String> {
    let guard = state.workers.0.lock().await;
    if guard.is_some() {
        return Err("请等待执行链清理结束后再删除记录".into());
    }
    let store = Arc::clone(&state.store);
    Ok(notify(
        &app,
        blocking(move || store.delete_history(&run_id, confirmed)).await?,
    ))
}

#[tauri::command]
pub async fn refresh_snapshot(
    state: State<'_, AppState>,
    run_id: String,
) -> Result<RunSnapshot, String> {
    let store = Arc::clone(&state.store);
    blocking(move || store.snapshot(&run_id)).await
}
#[tauri::command]
pub async fn swap_roles(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: String,
) -> Result<RunSnapshot, String> {
    let guard = state.workers.0.lock().await;
    if guard.is_some() {
        return Err("执行链运行中，不能交换角色".into());
    }
    let store = Arc::clone(&state.store);
    Ok(notify(
        &app,
        blocking(move || store.swap_roles(&run_id)).await?,
    ))
}
#[tauri::command]
pub async fn set_workspace(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: String,
    workspace: String,
) -> Result<RunSnapshot, String> {
    let guard = state.workers.0.lock().await;
    if guard.is_some() {
        return Err("执行链运行中，不能切换工作区".into());
    }
    let store = Arc::clone(&state.store);
    Ok(notify(
        &app,
        blocking(move || store.set_workspace(&run_id, &workspace)).await?,
    ))
}
#[tauri::command]
pub async fn pause_run(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: String,
) -> Result<RunSnapshot, String> {
    let guard = state.workers.0.lock().await;
    if guard.is_some() {
        return Err("请使用停止当前执行".into());
    }
    let store = Arc::clone(&state.store);
    Ok(notify(
        &app,
        blocking(move || {
            if store.snapshot(&run_id)?.run.state != RunState::Ready {
                return Err("Only an idle run can be paused in this foundation release".into());
            }
            store.transition(
                &run_id,
                RunState::Paused,
                "Operator paused the idle run. No worker is running.",
            )
        })
        .await?,
    ))
}
#[tauri::command]
pub async fn acknowledge_recovery(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: String,
) -> Result<RunSnapshot, String> {
    let guard = state.workers.0.lock().await;
    if guard.is_some() {
        return Err("请等待执行链清理结束".into());
    }
    let store: Arc<Store> = Arc::clone(&state.store);
    Ok(notify(
        &app,
        blocking(move || store.acknowledge_recovery(&run_id)).await?,
    ))
}
