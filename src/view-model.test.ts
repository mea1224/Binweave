import { describe, expect, it } from 'vitest';
import { eventContent, formatTime, label, newerSnapshot, resolveEdges } from './view-model';
import { serverName, systemText } from './localization';
import type { AgentEvent, RunSnapshot } from './types';
const event = (id: string, sequence: number): AgentEvent => ({ id, sequence, runId: 'run', createdAt: '2026-11-19T10:00:00Z', agent: null, kind: 'system', content: id, parentId: null });
const snapshot = (updatedAt: string, events: AgentEvent[]): RunSnapshot => ({ run: { id: 'run', title: 'Run', workspace: '/test', createdAt: updatedAt, updatedAt, state: 'ready', roles: { analyst: 'codex', implementer: 'claude', revision: 1 } }, checkpoint: { runId: 'run', state: 'ready', activeAgent: null, nextStep: 'Review', lastEventId: null, updatedAt, roleRevision: 1, interruptedState: null }, events, edges: [], artifacts: [] });
describe('workspace view model', () => {
  it('accepts deletion and rejects delayed snapshots from before deletion', () => {
    const old = snapshot('2026-11-19T11:00:00Z', [event('1', 1)]);
    const cleared = { ...old, events: [], run: { ...old.run, execution: { historyRevision: 1, chainActive: false, codexSession: null, claudeSession: null, turns: [] } } };
    expect(newerSnapshot(old, cleared)).toBe(cleared);
    expect(newerSnapshot(cleared, old)).toBe(cleared);
  });
  it('keeps newer snapshots when IPC and event results arrive out of order', () => {
    const current = snapshot('2026-11-19T11:00:00Z', [event('1', 1), event('2', 2)]);
    expect(newerSnapshot(current, snapshot('2026-11-19T10:00:00Z', []))).toBe(current);
    expect(newerSnapshot(null, current)).toBe(current);
  });
  it('resolves actual causal edges and omits dangling links', () => {
    const edges = [{ id: 'a', runId: 'run', source: '1', target: '2', edgeType: 'skill_revision' as const }, { id: 'b', runId: 'run', source: 'missing', target: '2', edgeType: 'sequence' as const }];
    const linked = resolveEdges([event('1', 1), event('2', 2)], edges);
    expect(linked).toHaveLength(1);
    expect(linked[0].target.sequence).toBe(2);
  });
  it('renders Chinese dates and labels without altering unknown identifiers', () => {
    expect(formatTime('invalid')).toBe('未知时间');
    expect(formatTime('2026-11-19T10:00:00Z')).toContain('11月');
    expect(label('updating_skill')).toBe('更新技能中');
    expect(label('not_checked')).toBe('未检查');
    expect(label('future_state')).toBe('future_state');
  });
  it('translates stored system events but never operator notes or agent output', () => {
    const content = 'Workspace initialized. No agent has been launched.';
    const original = { ...event('1', 1), content };
    expect(eventContent(original)).toBe('工作区已初始化，尚未启动任何智能体。');
    expect(original.content).toBe(content);
    expect(eventContent({ ...original, kind: 'message' })).toBe(content);
    expect(eventContent({ ...original, agent: 'codex' })).toBe(content);
    expect(eventContent({ ...original, kind: 'stdout' })).toBe(content);
  });
  it('translates checkpoints, transitions and artifact summaries without changing paths', () => {
    const nextStep = 'Operator paused the idle run. No worker is running.';
    expect(systemText(`Ready -> Paused: ${nextStep}`)).toBe('就绪 → 已暂停：操作员已暂停空闲任务，当前没有工作进程运行。');
    const interrupted = 'Interrupted during Analyzing: Review diagnostics and role contract. Review before resuming; no work has been replayed.';
    expect(systemText(interrupted)).toContain('任务在“分析中”阶段中断：请检查环境诊断和角色契约。');
    expect(systemText('Role contract v3 saved: artifacts/run/skills/v3/SKILL.md')).toBe('角色契约 v3 已保存：artifacts/run/skills/v3/SKILL.md');
    expect(systemText('Custom next step')).toBe('Custom next step');
  });
  it('preserves diagnostic values and translates known error and scope wrappers', () => {
    expect(systemText('Version check passed: codex-cli 0.170.0. Authentication and model access are not verified.')).toContain('codex-cli 0.170.0');
    expect(systemText('MCP request failed with JSON-RPC code -32600')).toBe('MCP 请求失败，JSON-RPC 错误码：-32600。');
    expect(systemText('Project MCP command detected but not executed. Explicit project trust is required.')).toContain('需先明确信任该项目');
    expect(serverName('analysis (Claude, project)')).toBe('analysis （Claude，项目）');
    expect(serverName('my-custom-REA')).toBe('my-custom-REA');
  });
});