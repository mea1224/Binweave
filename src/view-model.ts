import type { AgentEvent, CausalEdge, CheckStatus, EdgeType, EventKind, RunSnapshot, RunState } from './types';
import { systemText } from './localization';

const labels: Record<RunState | EventKind | EdgeType | CheckStatus | 'analyst' | 'implementer', string> = {
  ready: '就绪', analyzing: '分析中', implementing: '实现中', verifying: '验证中',
  updating_skill: '更新技能中', paused: '已暂停', interrupted: '已中断', completed: '已完成', failed: '失败',
  prompt: '收到的提示词', message: '消息', tool_summary: '工具摘要', skill_update: '技能更新', state_transition: '状态变更',
  system: '系统', stdout: '标准输出', stderr: '标准错误',
  sequence: '先后顺序', handoff: '任务交接', verification: '验证', skill_revision: '技能修订', recovery: '恢复',
  ok: '正常', warning: '需注意', error: '错误', not_checked: '未检查', analyst: '分析员', implementer: '实现员',
};
export const label = (value: string): string => Object.hasOwn(labels, value) ? labels[value as keyof typeof labels] : value;
export function eventContent(event: AgentEvent): string {
  // Never translate operator notes or agent output, even if they match system text.
  return event.agent === null && ['system', 'state_transition', 'skill_update'].includes(event.kind)
    ? systemText(event.content) : event.content;
}
export function formatTime(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '未知时间' : date.toLocaleString('zh-CN', { month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
}
export function newerSnapshot(current: RunSnapshot | null, next: RunSnapshot): RunSnapshot {
  if (!current || current.run.id !== next.run.id) return next;
  const revision = current.run.execution?.historyRevision ?? 0;
  const nextRevision = next.run.execution?.historyRevision ?? 0;
  if (revision !== nextRevision) return nextRevision > revision ? next : current;
  if (Date.parse(next.run.updatedAt) < Date.parse(current.run.updatedAt)) return current;
  if (next.events.length < current.events.length) return current;
  return next;
}
export function resolveEdges(events: AgentEvent[], edges: CausalEdge[]) {
  const byId = new Map(events.map(event => [event.id, event]));
  return edges.flatMap(edge => {
    const source = byId.get(edge.source);
    const target = byId.get(edge.target);
    return source && target ? [{ edge, source, target }] : [];
  });
}