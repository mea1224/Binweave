use super::*;

impl Store {
    pub fn delete_history(&self, id: &str, confirmed: bool) -> Result<RunSnapshot, String> {
        if !confirmed {
            return Err("请先确认删除；Codex／Claude 自身的历史记录仍旧存在。".into());
        }
        let mut db = self.db()?;
        let mut snapshot = load(&db, id)?;
        if snapshot.run.execution.chain_active
            || matches!(
                snapshot.run.state,
                RunState::Analyzing
                    | RunState::Implementing
                    | RunState::Verifying
                    | RunState::UpdatingSkill
            )
            || snapshot
                .run
                .execution
                .turns
                .iter()
                .any(|t| matches!(t.status, TurnStatus::Submitted | TurnStatus::Running))
        {
            return Err("请先停止任务并等待进程清理完成，再删除记录。".into());
        }
        let revision = snapshot
            .run
            .execution
            .history_revision
            .checked_add(1)
            .ok_or("记录版本超出范围")?;
        snapshot.run.execution = Execution {
            elicitation_policy: snapshot.run.execution.elicitation_policy,
            history_revision: revision,
            ..Execution::default()
        };
        snapshot.run.updated_at = Utc::now();
        snapshot.events.clear();
        snapshot.edges.clear();
        snapshot.checkpoint.last_event_id = None;
        snapshot.checkpoint.active_agent = None;
        snapshot.checkpoint.logical_role = None;
        snapshot.checkpoint.provider = None;
        snapshot.checkpoint.updated_at = snapshot.run.updated_at;
        snapshot.checkpoint.next_step =
            if matches!(snapshot.run.state, RunState::Paused | RunState::Interrupted) {
                "本地记录已删除；请检查工作区文件并确认恢复检查点后再发送指令。"
            } else {
                "本地记录已删除；下次指令将建立新的 Agent 会话。"
            }
            .into();
        // No filesystem or CLI-history deletion. Keep role/skill artifacts intact.
        let tx = db.transaction().map_err(err)?;
        tx.execute("DELETE FROM run_edges WHERE run_id=?1", [id])
            .map_err(err)?;
        tx.execute("DELETE FROM run_events WHERE run_id=?1", [id])
            .map_err(err)?;
        tx.execute(
            "UPDATE sessions SET updated_at=?2,payload=?3 WHERE id=?1",
            params![
                id,
                snapshot.run.updated_at.to_rfc3339(),
                json(&snapshot.run)?
            ],
        )
        .map_err(err)?;
        tx.execute(
            "UPDATE run_checkpoints SET payload=?2 WHERE run_id=?1",
            params![id, json(&snapshot.checkpoint)?],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletion_requires_confirmation_and_idle_worker_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let initial = store
            .ensure_active_run(&dir.path().to_string_lossy())
            .unwrap();
        let id = &initial.run.id;
        store
            .begin_prompt(id, AgentId::Codex, "原任务", None)
            .unwrap();
        assert!(store.delete_history(id, true).is_err());
        store
            .finish_prompt(id, AgentId::Codex, true, "完成")
            .unwrap();
        assert!(store.delete_history(id, true).is_err());
        store.end_chain(id, None, false).unwrap();
        assert!(store.delete_history(id, false).is_err());
        let before = store.snapshot(id).unwrap();
        assert!(!before.events.is_empty());
        let deleted = store.delete_history(id, true).unwrap();
        assert!(deleted.events.is_empty() && deleted.edges.is_empty());
        assert!(deleted.run.execution.turns.is_empty());
        assert!(deleted.run.execution.codex_session.is_none());
        assert_eq!(deleted.run.workspace, before.run.workspace);
        assert_eq!(deleted.artifacts.len(), before.artifacts.len());
        assert_eq!(deleted.run.execution.history_revision, 1);
        drop(store);
        let reopened = Store::open(dir.path()).unwrap();
        assert!(reopened.snapshot(id).unwrap().events.is_empty());
        reopened
            .begin_prompt(id, AgentId::Claude, "新任务", None)
            .unwrap();
    }

    #[test]
    fn deletion_rolls_back_on_database_failure() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let s = store
            .ensure_active_run(&dir.path().to_string_lossy())
            .unwrap();
        store.db().unwrap().execute_batch("CREATE TRIGGER fail_delete BEFORE UPDATE ON sessions BEGIN SELECT RAISE(ABORT, 'fixture'); END;").unwrap();
        assert!(store.delete_history(&s.run.id, true).is_err());
        let saved = store.snapshot(&s.run.id).unwrap();
        assert_eq!(saved.events.len(), s.events.len());
        assert_eq!(saved.edges.len(), s.edges.len());
        assert_eq!(saved.run.execution.history_revision, 0);
    }
}
