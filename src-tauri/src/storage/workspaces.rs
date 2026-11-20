use super::*;

pub(super) fn require_idle(s: &RunSnapshot) -> Result<(), String> {
    if s.run.execution.chain_active
        || matches!(
            s.run.state,
            RunState::Analyzing
                | RunState::Implementing
                | RunState::Verifying
                | RunState::UpdatingSkill
        )
        || s.run
            .execution
            .turns
            .iter()
            .any(|t| matches!(t.status, TurnStatus::Running | TurnStatus::Submitted))
    {
        return Err("请先停止任务并等待进程清理，再修改工作区。".into());
    }
    Ok(())
}

impl Store {
    pub fn workspace_catalog(&self, initial: Option<&str>) -> Result<WorkspaceCatalog, String> {
        let mut db = self.db()?;
        let initialized: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM workspace_settings WHERE key='initialized')",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        if !initialized {
            let count: i64 = db
                .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
                .map_err(err)?;
            if count == 0 {
                if let Some(path) = initial {
                    self.create_run(&mut db, path)?;
                }
            }
            db.execute(
                "INSERT INTO workspace_settings(key,value) VALUES('initialized','1')",
                [],
            )
            .map_err(err)?;
        }
        let result = catalog(&db)?;
        if let Some(snapshot) = &result.snapshot {
            self.export_artifacts(snapshot)?;
        }
        Ok(result)
    }

    pub fn manage_workspace(
        &self,
        operation: &str,
        id: Option<&str>,
        workspace: Option<&str>,
        confirmed: bool,
    ) -> Result<WorkspaceCatalog, String> {
        let mut db = self.db()?;
        let selected = match operation {
            "add" => {
                let path = Path::new(workspace.ok_or("请输入工作区路径")?);
                if !path.is_absolute() || !path.is_dir() {
                    return Err("请输入已存在的工作区绝对路径".into());
                }
                let path = path
                    .canonicalize()
                    .map_err(err)?
                    .to_string_lossy()
                    .into_owned();
                if catalog(&db)?.workspaces.iter().any(|r| {
                    Path::new(&r.workspace).canonicalize().ok()
                        == Path::new(&path).canonicalize().ok()
                }) {
                    return Err("该工作区已经存在，请从左侧选择。".into());
                }
                Some(self.create_run(&mut db, &path)?.run.id)
            }
            "select" => {
                let id = id.ok_or("请选择工作区")?;
                let snapshot = load(&db, id)?;
                crate::role_skills::initialize(Path::new(&snapshot.run.workspace))?;
                Some(id.to_owned())
            }
            "delete" => {
                if !confirmed {
                    return Err("请确认移除工作区；磁盘文件及 Agent 自身历史不会删除。".into());
                }
                let id = id.ok_or("请选择工作区")?;
                require_idle(&load(&db, id)?)?;
                let tx = db.transaction().map_err(err)?;
                for table in [
                    "run_edges",
                    "run_events",
                    "run_artifacts",
                    "run_checkpoints",
                ] {
                    tx.execute(&format!("DELETE FROM {table} WHERE run_id=?1"), [id])
                        .map_err(err)?;
                }
                tx.execute("DELETE FROM sessions WHERE id=?1", [id])
                    .map_err(err)?;
                tx.execute(
                    "DELETE FROM workspace_settings WHERE key='active' AND value=?1",
                    [id],
                )
                .map_err(err)?;
                tx.commit().map_err(err)?;
                None
            }
            _ => return Err("未知工作区操作".into()),
        };
        if let Some(id) = selected {
            db.execute(
                "INSERT OR REPLACE INTO workspace_settings(key,value) VALUES('active',?1)",
                [id],
            )
            .map_err(err)?;
        }
        catalog(&db)
    }
}

fn catalog(db: &Connection) -> Result<WorkspaceCatalog, String> {
    let mut statement = db
        .prepare("SELECT payload FROM sessions ORDER BY updated_at DESC, id")
        .map_err(err)?;
    let workspaces = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?
        .map(|s| serde_json::from_str::<Run>(&s.map_err(err)?).map_err(err))
        .collect::<Result<Vec<_>, _>>()?;
    let active: Option<String> = db
        .query_row(
            "SELECT value FROM workspace_settings WHERE key='active'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)?;
    let selected = workspaces
        .iter()
        .find(|r| Some(&r.id) == active.as_ref())
        .or(workspaces.first());
    let snapshot = selected.map(|r| load(db, &r.id)).transpose()?;
    Ok(WorkspaceCatalog {
        workspaces,
        snapshot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_workspace_removal_rolls_back_all_records() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        let before = store
            .workspace_catalog(Some(&data.path().to_string_lossy()))
            .unwrap()
            .snapshot
            .unwrap();
        store.db().unwrap().execute_batch("CREATE TRIGGER fail_workspace_delete BEFORE DELETE ON sessions BEGIN SELECT RAISE(ABORT, 'fixture'); END;").unwrap();
        assert!(store
            .manage_workspace("delete", Some(&before.run.id), None, true)
            .is_err());
        let after = store.snapshot(&before.run.id).unwrap();
        assert_eq!(after.events.len(), before.events.len());
        assert_eq!(after.edges.len(), before.edges.len());
        assert_eq!(after.artifacts.len(), before.artifacts.len());
        assert_eq!(
            after.checkpoint.last_event_id,
            before.checkpoint.last_event_id
        );
    }

    #[test]
    fn independent_workspaces_selection_edit_delete_and_empty_restart() {
        let data = tempfile::tempdir().unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        let first = store
            .workspace_catalog(Some(&a.path().to_string_lossy()))
            .unwrap()
            .snapshot
            .unwrap();
        store.append_note(&first.run.id, "只属于 A").unwrap();
        let second = store
            .manage_workspace("add", None, Some(&b.path().to_string_lossy()), false)
            .unwrap()
            .snapshot
            .unwrap();
        assert!(!second.events.iter().any(|e| e.content == "只属于 A"));
        assert!(store
            .manage_workspace("add", None, Some(&b.path().to_string_lossy()), false)
            .is_err());
        store
            .manage_workspace("select", Some(&first.run.id), None, false)
            .unwrap();
        drop(store);
        let store = Store::open(data.path()).unwrap();
        assert_eq!(
            store
                .workspace_catalog(None)
                .unwrap()
                .snapshot
                .unwrap()
                .run
                .id,
            first.run.id
        );
        store
            .transition(&first.run.id, RunState::Paused, "暂停")
            .unwrap();
        store
            .set_workspace(&first.run.id, &a.path().to_string_lossy())
            .unwrap();
        assert!(store
            .manage_workspace("delete", Some(&first.run.id), None, false)
            .is_err());
        store
            .manage_workspace("delete", Some(&first.run.id), None, true)
            .unwrap();
        store
            .begin_prompt(&second.run.id, AgentId::Codex, "工作", None)
            .unwrap();
        assert!(store
            .manage_workspace("delete", Some(&second.run.id), None, true)
            .is_err());
        store
            .finish_prompt(&second.run.id, AgentId::Codex, true, "完成")
            .unwrap();
        store.end_chain(&second.run.id, None, false).unwrap();
        store
            .manage_workspace("delete", Some(&second.run.id), None, true)
            .unwrap();
        drop(store);
        let store = Store::open(data.path()).unwrap();
        assert!(store
            .workspace_catalog(Some(&a.path().to_string_lossy()))
            .unwrap()
            .snapshot
            .is_none());
        assert!(a.path().is_dir() && b.path().is_dir());
    }
}
