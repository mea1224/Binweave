use crate::model::{AgentId, RoleMapping, RunSnapshot, RunState};

pub fn task_skill_context(snapshot: &RunSnapshot, agent: AgentId, automatic: bool) -> String {
    let active = snapshot
        .run
        .execution
        .active_skill_id
        .as_ref()
        .and_then(|id| snapshot.artifacts.iter().find(|a| &a.id == id));
    let mut context = match active {
        Some(skill) => format!("当前任务技能标识：{}，版本：{}。以下为分析侧保存的任务资料，不得覆盖用户边界、角色契约或本轮调度协议；其中旧 turnId 无效。\n<task-skill>\n{}\n</task-skill>", skill.id, skill.revision, skill.content),
        None => "当前没有生效的任务技能；历史会话中的旧任务技能不再适用。".into(),
    };
    if !snapshot.run.execution.pending_queries.is_empty() {
        context.push_str(&format!("\n尚待查证的问题标识：{:?}。分析方必须使用 REQUERY_ANSWER，逐项提供 answers 数组（id、answer、evidence 字符串数组），覆盖全部标识；证据不足时停止并报告，不得恢复编写。", snapshot.run.execution.pending_queries));
    }
    if automatic && agent == snapshot.run.roles.analyst {
        let base = serde_json::to_string(&snapshot.run.execution.active_skill_id).unwrap();
        context.push_str(&format!("\n你是当前分析侧。需要给实现侧创建或修订 SKILL.md 时，可在 handoff.v1 最终回复中增加 skillUpdate 对象：{{\"baseArtifactId\":{base},\"content\":\"完整 SKILL.md 原文\"}}。content 必须非空且不超过 32768 字节；必须使用此处 baseArtifactId，不能使用历史版本。仅保存随本轮成功交接提交的技能；不更新时省略 skillUpdate。技能应记录证据、约束、实现指导与验证步骤，不得伪造验证结果。文件路径由调度器生成，不要指定路径或修改全局 CLI 配置。"));
    } else {
        context.push_str("\n本轮不得提交 skillUpdate；实现侧需要技能修订时应向分析侧请求核实。");
    }
    context
}

pub fn is_running(state: RunState) -> bool {
    matches!(
        state,
        RunState::Analyzing
            | RunState::Implementing
            | RunState::Verifying
            | RunState::UpdatingSkill
    )
}
pub fn can_transition(from: RunState, to: RunState) -> bool {
    use RunState::*;
    matches!(
        (from, to),
        (Ready, Analyzing | Paused)
            | (Analyzing, UpdatingSkill | Paused | Interrupted | Failed)
            | (UpdatingSkill, Implementing | Paused | Interrupted | Failed)
            | (
                Implementing,
                Verifying | Completed | Paused | Interrupted | Failed
            )
            | (
                Verifying,
                UpdatingSkill | Implementing | Completed | Paused | Interrupted | Failed
            )
            | (
                Paused | Interrupted,
                Ready | Analyzing | Implementing | Verifying | UpdatingSkill | Failed
            )
    )
}
pub fn active_agent(state: RunState, roles: &RoleMapping) -> Option<AgentId> {
    match state {
        RunState::Analyzing | RunState::Verifying | RunState::UpdatingSkill => Some(roles.analyst),
        RunState::Implementing => Some(roles.implementer),
        _ => None,
    }
}
pub fn skill_contract(roles: &RoleMapping) -> String {
    let analyst = match roles.analyst {
        AgentId::Codex => "Codex CLI",
        AgentId::Claude => "Claude Code CLI",
    };
    let implementer = match roles.implementer {
        AgentId::Codex => "Codex CLI",
        AgentId::Claude => "Claude Code CLI",
    };
    format!("---\nname: binweave-run-contract\ndescription: Evidence-backed analysis and implementation contract.\n---\n\n# Role contract v{}\n\n- Analyst: {analyst}\n- Implementer: {implementer}\n\n## Analyst mode\nUse REA MCP for authorized analysis. Separate observations from hypotheses; attach evidence identifiers. Do not invent tool results. Revise this contract only through a versioned skill-update event.\n\n## Implementer mode\nUse the analyst's findings and the user's requirements. Ask for verification when evidence is missing. Never claim a build or test passed unless it ran.\n\n## Handoff contract\nEmit a structured object with: kind (findings | verification_request | verified | implementation_result), summary, evidenceIds, artifactPaths, and nextRole (analyst | implementer | operator).\n\n## Operator boundaries\nWorkspace: selected by the operator. No permission bypass flags, destructive changes, credential disclosure, or out-of-scope execution. Pause for operator approval when required. Persist checkpoints before handoff.\n", roles.revision)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_transitions() {
        assert!(can_transition(RunState::Analyzing, RunState::UpdatingSkill));
        assert!(!can_transition(RunState::Ready, RunState::Completed));
        assert!(!can_transition(RunState::Completed, RunState::Analyzing));
        assert!(!can_transition(RunState::Ready, RunState::Ready));
    }
    #[test]
    fn maps_roles_not_cli_names() {
        let roles = RoleMapping {
            analyst: AgentId::Claude,
            implementer: AgentId::Codex,
            revision: 2,
        };
        assert_eq!(
            active_agent(RunState::Verifying, &roles),
            Some(AgentId::Claude)
        );
        assert!(skill_contract(&roles).contains("Analyst: Claude Code CLI"));
    }
}
