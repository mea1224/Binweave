use super::*;

pub(super) fn add_task_skill(s: &mut RunSnapshot, source_id: &str, content: &str) {
    let revision = s.artifacts.iter().map(|a| a.revision).max().unwrap_or(0) + 1;
    let id = Uuid::new_v4().to_string();
    let path = format!("artifacts/{}/task-skills/v{revision}/SKILL.md", s.run.id);
    s.artifacts.push(Artifact {
        id: id.clone(),
        run_id: s.run.id.clone(),
        revision,
        relative_path: path.clone(),
        content: content.into(),
        created_at: Utc::now(),
    });
    s.run.execution.active_skill_id = Some(id.clone());
    push_event(
        s,
        Some(s.run.roles.analyst),
        EventKind::SkillUpdate,
        &format!(
            "分析侧任务技能 v{revision} 已保存：{path}；产物标识：{id}；角色版本：{}",
            s.run.roles.revision
        ),
        EdgeType::SkillRevision,
    );
    let event = s.events.last_mut().unwrap();
    event.parent_id = Some(source_id.into());
    let target = event.id.clone();
    s.edges.retain(|e| e.target != target);
    s.edges.push(CausalEdge {
        identity: s.run.roles.stamp(Some(s.run.roles.analyst)),
        id: Uuid::new_v4().to_string(),
        run_id: s.run.id.clone(),
        source: source_id.into(),
        target,
        edge_type: EdgeType::SkillRevision,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        handoff::{Handoff, SkillUpdate},
        process::parser::ParsedOutput,
    };

    fn reply(
        store: &Store,
        s: &RunSnapshot,
        base: Option<String>,
        evidence: Vec<String>,
        success: bool,
    ) -> String {
        let sender = s.run.execution.turns.last().unwrap().agent;
        let request = Handoff {
            binweave: "handoff.v1".into(),
            turn_id: s.run.execution.turns.last().unwrap().prompt_id.clone(),
            to: if sender == AgentId::Codex {
                AgentId::Claude
            } else {
                AgentId::Codex
            },
            instruction: "实现并验证".into(),
            reason: "证据已整理".into(),
            evidence_ids: evidence,
            skill_update: Some(SkillUpdate {
                base_artifact_id: base,
                content: "# 任务技能\n保留原文；验证边界。".into(),
            }),
        };
        let output = ParsedOutput {
            interaction: None,
            resolved_interaction: None,
            kind: EventKind::Message,
            raw: "fixture".into(),
            summary: Some(serde_json::to_string(&request).unwrap()),
            session_id: None,
            reported_failure: false,
        };
        let saved = store.append_output(&s.run.id, sender, &output).unwrap();
        store
            .finish_prompt(&s.run.id, sender, success, "测试完成")
            .unwrap();
        saved.events.last().unwrap().id.clone()
    }

    #[test]
    fn versions_skills_for_either_analyst_and_preserves_causal_history() {
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
            let analyst = initial.run.roles.analyst;
            let implementer = initial.run.roles.implementer;
            let mut base = None;
            for _ in 0..2 {
                let sent = store.begin_prompt(id, analyst, "分析", None).unwrap();
                let source = reply(
                    &store,
                    &sent,
                    base.clone(),
                    vec![sent.events.last().unwrap().id.clone()],
                    true,
                );
                let accepted = store
                    .begin_auto_prompt(id, implementer, "实现并验证", &source)
                    .unwrap();
                let artifact = accepted.artifacts.last().unwrap();
                assert_ne!(base.as_ref(), Some(&artifact.id));
                assert_eq!(
                    accepted.run.execution.active_skill_id.as_ref(),
                    Some(&artifact.id)
                );
                assert_eq!(
                    std::fs::read_to_string(dir.path().join(&artifact.relative_path)).unwrap(),
                    artifact.content
                );
                let event = accepted
                    .events
                    .iter()
                    .find(|e| {
                        e.kind == EventKind::SkillUpdate && e.parent_id.as_deref() == Some(&source)
                    })
                    .unwrap();
                assert_eq!(event.agent, Some(analyst));
                assert!(accepted.edges.iter().any(|e| e.source == source
                    && e.target == event.id
                    && e.edge_type == EdgeType::SkillRevision));
                assert!(accepted
                    .edges
                    .iter()
                    .any(|e| e.source == source && e.edge_type == EdgeType::Handoff));
                let context = orchestrator::task_skill_context(&accepted, implementer, true);
                assert!(context.contains(&artifact.content));
                assert!(context.contains("本轮不得提交 skillUpdate"));
                base = Some(artifact.id.clone());
                store.finish_prompt(id, implementer, true, "完成").unwrap();
                store.end_chain(id, None, false).unwrap();
            }
            let restored = Store::open(dir.path()).unwrap().snapshot(id).unwrap();
            assert_eq!(restored.run.execution.active_skill_id, base);
            let count = restored.artifacts.len();
            let changed = store.swap_roles(id).unwrap();
            assert_eq!(changed.run.execution.active_skill_id, base);
            assert_eq!(changed.artifacts.len(), count + 1);
            assert!(
                orchestrator::task_skill_context(&changed, analyst, true).contains("# 任务技能")
            );
        }
    }

    #[test]
    fn rejects_invalid_updates_without_partial_state_changes() {
        for mode in [
            "implementer",
            "stale",
            "evidence",
            "failed",
            "export",
            "database",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(dir.path()).unwrap();
            let initial = store
                .ensure_active_run(&dir.path().to_string_lossy())
                .unwrap();
            let id = &initial.run.id;
            let sender = if mode == "implementer" {
                AgentId::Claude
            } else {
                AgentId::Codex
            };
            let target = if sender == AgentId::Codex {
                AgentId::Claude
            } else {
                AgentId::Codex
            };
            let sent = store.begin_prompt(id, sender, "分析", None).unwrap();
            let base = (mode == "stale").then(|| "old".into());
            let evidence = if mode == "evidence" {
                vec!["missing".into()]
            } else {
                vec![]
            };
            let source = reply(&store, &sent, base, evidence, mode != "failed");
            if mode == "export" {
                std::fs::write(
                    dir.path().join(format!("artifacts/{id}/task-skills")),
                    "blocked",
                )
                .unwrap();
            }
            let before = serde_json::to_value(store.snapshot(id).unwrap()).unwrap();
            if mode == "database" {
                store.db().unwrap().execute_batch("CREATE TRIGGER reject_skill BEFORE INSERT ON run_artifacts BEGIN SELECT RAISE(ABORT, 'fixture'); END;").unwrap();
            }
            assert!(
                store
                    .begin_auto_prompt(id, target, "实现并验证", &source)
                    .is_err(),
                "{mode}"
            );
            assert_eq!(
                before,
                serde_json::to_value(store.snapshot(id).unwrap()).unwrap(),
                "{mode}"
            );
        }
    }

    #[test]
    fn workspace_change_deactivates_but_keeps_skill_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let initial = store
            .ensure_active_run(&dir.path().to_string_lossy())
            .unwrap();
        let id = &initial.run.id;
        let sent = store
            .begin_prompt(id, AgentId::Codex, "分析", None)
            .unwrap();
        let source = reply(&store, &sent, None, vec![], true);
        store
            .begin_auto_prompt(id, AgentId::Claude, "实现并验证", &source)
            .unwrap();
        store
            .finish_prompt(id, AgentId::Claude, true, "完成")
            .unwrap();
        store.end_chain(id, None, false).unwrap();
        let changed = store
            .set_workspace(id, &dir.path().to_string_lossy())
            .unwrap();
        assert!(changed.run.execution.active_skill_id.is_none());
        assert_eq!(changed.artifacts.len(), 2);
    }
}
