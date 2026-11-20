import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import TurnProgress from './TurnProgress';
import type { Turn } from '../types';

describe('task progress and inline interaction', () => {
  it('shows idle and task elapsed time together and freezes completed duration', () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date('2026-11-20T12:03:05Z'));
      const conversation = { id: 'p', agent: 'codex' as const, createdAt: '2026-11-20T10:00:00Z', events: [{ id: 'e', runId: 'r', sequence: 1, createdAt: '2026-11-20T12:02:00Z', kind: 'message' as const, content: '原始输出', agent: 'codex' as const, parentId: null }] };
      const html = renderToStaticMarkup(<TurnProgress turn={{ ...turn, interactions: [] }} conversation={conversation} open active enabled />);
      expect(html).toContain('距最近活动 1 分 5 秒');
      expect(html).toContain('任务已用时 2 小时 3 分');
      vi.setSystemTime(new Date('2026-11-21T12:00:00Z'));
      const done = renderToStaticMarkup(<TurnProgress turn={{ ...turn, status: 'completed', interactions: [] }} conversation={conversation} open active={false} enabled />);
      expect(done).toContain('任务用时 2 小时 2 分');
      expect(done).not.toContain('距最近活动');
    } finally { vi.useRealTimers(); }
  });
  const turn: Turn = { promptId: 'p', agent: 'codex', status: 'running', detail: '', interactions: [{ id: 'q', title: '请选择', detail: '请求原文', approval: false, status: 'pending', questions: [{ id: 'plan', text: '选择计划', options: ['A', 'B'], multiple: true }] }] };
  const render = (t: Turn, active = true) => renderToStaticMarkup(<TurnProgress turn={t} conversation={{ id: 'p', agent: 'codex', createdAt: '', events: [] }} open={false} active={active} enabled respond={async () => true} />);
  it('shows choices and text inside even a collapsed card', () => {
    const html = render(turn);
    expect(html).toContain('等待用户回答');
    expect(html).toContain('显示进度');
    expect(html).toContain('内容已收起');
    expect(html).toContain('type="checkbox"');
    expect(html).toContain('手动输入回答');
    expect(html).not.toContain('agent-prompt');
  });
  it('offers explicit allow and deny choices for permission requests', () => {
    const html = render({ ...turn, interactions: [{ id: 'approval', title: '请求授权：Bash', detail: '{"command":"echo test"}', approval: true, status: 'pending', questions: [] }] });
    expect(html).toContain('允许本次请求');
    expect(html).toContain('拒绝');
    expect(html).toContain('等待用户授权');
    expect(html).toContain('选择将回传当前 Agent');
    expect(html).toContain('暂停 5 分钟思考计时');
  });
  it('隐藏已回答的 MCP 交互但保留同轮其他待处理请求', () => {
    for (const agent of ['codex', 'claude'] as const) {
      const request = { id: 'done', title: '已选择的 MCP 请求', detail: '已选择请求的原文', kind: 'mcp_confirmation' as const, approval: false, status: 'answered' as const, decision: 'accept', delivery: 'delivered', questions: [] };
      const html = render({ ...turn, agent, interactions: [request, { ...request, id: 'next', title: '新的 MCP 请求', detail: '新的请求原文', status: 'pending', decision: null, delivery: null }] });
      expect(html).not.toContain('已选择的 MCP 请求');
      expect(html).not.toContain('已选择请求的原文');
      expect(html).toContain('新的 MCP 请求');
      expect(html).toContain('等待用户回答');
      expect(html.match(/aria-label="MCP 用户交互"/g)).toHaveLength(1);
    }
  });
  it('hides settled approval history without hiding pending requests or other interactions', () => {
    for (const agent of ['codex', 'claude'] as const) {
      for (const active of [true, false]) {
        const approval = { id: 'old', title: '已处理授权', detail: 'echo approved-command', approval: true, status: 'answered' as const, questions: [] };
        const html = render({ ...turn, agent, status: active ? 'running' : 'completed', interactions: [
          approval,
          { ...approval, id: 'auto', canRemember: true, title: '已按记住的命令自动允许', detail: 'echo remembered-command' },
          { ...approval, id: 'pending', status: 'pending', title: '待处理授权', detail: 'echo pending-command' },
          { ...approval, id: 'expired', status: 'expired', title: '过期授权', detail: 'echo expired-command' },
          { ...turn.interactions![0], status: 'answered' },
        ] }, active);
        expect(html).not.toContain('已处理授权');
        expect(html).not.toContain('approved-command');
        expect(html).not.toContain('已按记住的命令自动允许');
        expect(html).not.toContain('remembered-command');
        expect(html).toContain('待处理授权');
        expect(html).toContain('pending-command');
        expect(html).toContain('过期授权');
        expect(html).toContain('请求已过期');
        expect(html).not.toContain('请求原文');
      }
    }
    const settled = render({ ...turn, interactions: [{ id: 'done', title: '已处理授权', detail: 'echo done', approval: true, status: 'answered', questions: [] }] });
    expect(settled).not.toContain('agent-interaction');
    expect(settled).not.toContain('等待用户授权');
  });
  it('distinguishes errors, active work and stopped requests', () => {
    const approval = { id: 'approval', title: '请求授权', detail: 'echo test', approval: true, canRemember: true, status: 'pending' as const, questions: [] };
    const remembered = render({ ...turn, interactions: [approval] });
    expect(remembered).toContain('允许，此命令不再询问');
    expect(remembered).toContain('重启应用后失效');
    expect(render({ ...turn, interactions: [{ ...approval, canRemember: false }] })).not.toContain('允许，此命令不再询问');
    expect(render({ ...turn, interactions: [{ ...approval, status: 'answered' }] })).not.toContain('<form');
    expect(render({ ...turn, interactions: [] })).toContain('turn-progress waiting');
    expect(render({ ...turn, interactions: [] })).toContain('正在思考');
    expect(render({ ...turn, interactions: [] })).toContain('thinking-spinner');
    expect(render({ ...turn, status: 'failed', detail: '错误代码 PROCESS_EXIT：退出码 17' }, false)).toContain('turn-progress error');
    expect(render({ ...turn, status: 'failed', detail: '错误代码 PROCESS_EXIT：退出码 17' }, false)).toContain('退出码 17');
    expect(render({ ...turn, status: 'cancelled' }, false)).toContain('用户已停止');
    expect(render(turn, false)).toContain('disabled=""');
    expect(render({ ...turn, interactions: turn.interactions!.map(r => ({ ...r, status: 'expired' })) }, false)).not.toContain('<form');
  });
  it('only shows recent work on failure or unexpected interruption', () => {
    for (const status of ['running', 'completed', 'cancelled', 'failed', 'interrupted'] as const) {
      const html = renderToStaticMarkup(<TurnProgress turn={{ ...turn, status, interactions: [] }} conversation={{ id: 'p', agent: 'codex', createdAt: '', events: [{ id: 'e', runId: 'r', sequence: 1, createdAt: new Date().toISOString(), kind: 'message', content: '原始输出', agent: 'codex', parentId: null }] }} open active={status === 'running'} enabled />);
      expect(html.includes('最近工作进展')).toBe(['failed', 'interrupted'].includes(status));
      if (status === 'running') expect(html).toContain('正在输出');
      else expect(html).not.toContain('thinking-spinner');
    }
  });
});