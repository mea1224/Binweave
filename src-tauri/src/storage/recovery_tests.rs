use super::*;

#[test]
fn only_last_conversation_can_retry_and_repeated_attempts_keep_original_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let s = store
        .ensure_active_run(&dir.path().to_string_lossy())
        .unwrap();
    let id = &s.run.id;
    let agent = s.run.roles.analyst;
    let other = s.run.roles.implementer;
    let older = store.begin_prompt(id, other, "另一侧旧任务", None).unwrap();
    let older_prompt = older.events.last().unwrap().id.clone();
    store.finish_prompt(id, other, true, "已完成").unwrap();
    store.end_chain(id, None, false).unwrap();
    let original = "原始问题 Q1\n保留文本与标识";
    let first = store.begin_prompt(id, agent, original, None).unwrap();
    let prompt = first.events.last().unwrap().id.clone();
    store.finish_prompt(id, agent, true, "已完成").unwrap();
    store.end_chain(id, None, false).unwrap();
    assert!(store.restart_turn(id, other, &older_prompt).is_err());
    assert!(store.restart_turn(id, agent, "不存在").is_err());
    let retry = store.restart_turn(id, agent, &prompt).unwrap();
    assert_eq!(retry.events.last().unwrap().content, original);
    assert_eq!(
        retry.events.last().unwrap().parent_id.as_deref(),
        Some(prompt.as_str())
    );
    assert_eq!(retry.run.execution.turns.len(), 3);
    assert!(store.restart_turn(id, agent, &prompt).is_err());
    store.finish_prompt(id, agent, true, "重试完成").unwrap();
    store.end_chain(id, None, false).unwrap();
    let again = store.restart_turn(id, agent, &prompt).unwrap();
    assert_eq!(again.events.last().unwrap().content, original);
    store.finish_prompt(id, agent, true, "完成").unwrap();
    store.end_chain(id, None, false).unwrap();
    let reloaded = Store::open(dir.path()).unwrap().snapshot(id).unwrap();
    assert_eq!(
        reloaded
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Recovery && e.source == prompt)
            .count(),
        2
    );
    store.swap_roles(id).unwrap();
    assert!(store.restart_turn(id, agent, &prompt).is_err());
}

#[test]
fn newer_card_blocks_previous_card_even_on_same_side() {
    for agent in [AgentId::Codex, AgentId::Claude] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let s = store
            .ensure_active_run(&dir.path().to_string_lossy())
            .unwrap();
        let id = &s.run.id;
        let old = store.begin_prompt(id, agent, "旧任务", None).unwrap();
        let old_id = old.events.last().unwrap().id.clone();
        store.finish_prompt(id, agent, true, "完成").unwrap();
        store.end_chain(id, None, false).unwrap();
        let new = store.begin_prompt(id, agent, "新任务", None).unwrap();
        let new_id = new.events.last().unwrap().id.clone();
        store.finish_prompt(id, agent, true, "完成").unwrap();
        store.end_chain(id, None, false).unwrap();
        assert!(store.restart_turn(id, agent, &old_id).is_err());
        assert!(store.restart_turn(id, agent, &new_id).is_ok());
    }
}

#[test]
fn isolated_restart_preserves_other_session_queries_and_history_after_reload() {
    for swapped in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut s = store
            .ensure_active_run(&dir.path().to_string_lossy())
            .unwrap();
        if swapped {
            s = store.swap_roles(&s.run.id).unwrap();
        }
        let analyst = s.run.roles.analyst;
        let other = s.run.roles.implementer;
        let id = s.run.id.clone();
        let started = store
            .begin_prompt(&id, analyst, "问题 Q1：查证指针；保留原始包", None)
            .unwrap();
        let prompt = started
            .run
            .execution
            .turns
            .last()
            .unwrap()
            .prompt_id
            .clone();
        assert!(store.restart_turn(&id, analyst, &prompt).is_err());
        store
            .finish_prompt(&id, analyst, false, "读取受阻")
            .unwrap();
        let mut saved = store.end_chain(&id, Some("人工接管"), false).unwrap();
        saved.run.execution.codex_session = Some("codex-session".into());
        saved.run.execution.claude_session = Some("claude-session".into());
        saved.run.execution.pending_queries = vec!["Q1".into()];
        save(&mut store.db().unwrap(), &saved).unwrap();
        assert!(store.restart_turn(&id, other, &prompt).is_err());
        assert_eq!(
            store.snapshot(&id).unwrap().run.execution.codex_session,
            saved.run.execution.codex_session
        );
        let retry = store.restart_turn(&id, analyst, &prompt).unwrap();
        assert_eq!(retry.run.execution.session(analyst), None);
        assert_eq!(
            retry.run.execution.session(other),
            saved.run.execution.session(other)
        );
        assert_eq!(retry.run.execution.pending_queries, ["Q1"]);
        assert_eq!(retry.run.execution.turns.len(), 2);
        assert!(retry
            .events
            .last()
            .unwrap()
            .content
            .contains("问题 Q1：查证指针；保留原始包"));
        assert!(retry
            .edges
            .iter()
            .any(|edge| edge.source == prompt && edge.edge_type == EdgeType::Recovery));
        assert!(store.restart_turn(&id, analyst, &prompt).is_err());
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        let loaded = store.snapshot(&id).unwrap();
        assert_eq!(loaded.run.execution.pending_queries, ["Q1"]);
        assert_eq!(
            loaded.run.execution.session(other),
            saved.run.execution.session(other)
        );
    }
}
