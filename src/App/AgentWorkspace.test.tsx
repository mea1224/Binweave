import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import AgentWorkspace, { handoffs, deliveryStatus, historicalRole, canRetryConversation } from './AgentWorkspace';
import type { AgentEvent, RunSnapshot } from '../types';
import { conversations } from './conversation';

const event = (id: string, agent: 'codex' | 'claude' | null, kind: AgentEvent['kind'] = 'message'): AgentEvent => ({ id, agent, kind, content: '<原文>', sequence: Number(id), runId: 'r', createdAt: '2026-11-19T10:00:00Z', parentId: null });
const snapshot: RunSnapshot = {
  run: { id: 'r', title: '', workspace: 'D:/test', createdAt: '', updatedAt: '', state: 'ready', roles: { analyst: 'claude', implementer: 'codex', revision: 2 } },
  checkpoint: { runId: 'r', state: 'ready', activeAgent: null, nextStep: '', lastEventId: null, updatedAt: '', roleRevision: 2, interruptedState: null }, artifacts: [],
  events: [event('1', 'codex'), event('2', 'claude', 'prompt'), event('3', 'claude'), event('4', 'codex', 'prompt'), event('5', null, 'system')],
  edges: [
    { id: 'a', runId: 'r', source: '1', target: '2', edgeType: 'handoff' },
    { id: 'b', runId: 'r', source: '3', target: '4', edgeType: 'handoff' },
    { id: 'c', runId: 'r', source: '4', target: '5', edgeType: 'handoff' },
    { id: 'd', runId: 'r', source: '2', target: '3', edgeType: 'sequence' },
  ],
};
const render = (value: RunSnapshot | null) => renderToStaticMarkup(<AgentWorkspace snapshot={value} enabled onSend={async () => true} onStop={async () => {}} />);
describe('conversation timeline and recipient selection', () => {
  it('已选择 MCP 请求及其系统审计不再显示在卡片，原始记录保持不变', () => {
    for (const agent of ['codex', 'claude'] as const) {
      for (const decision of ['accept', 'decline', 'cancel']) {
        const title = 'MCP 用户交互 · rea';
        const audit = `${title}：选择 ${decision}；投递状态 delivered（未保存表单回答）。`;
        const value: RunSnapshot = { ...snapshot, edges: [], events: [
          event('1', agent, 'prompt'),
          { ...event('2', agent, 'system'), content: `等待用户处理：${title}` },
          { ...event('3', agent, 'system'), content: audit },
          { ...event('4', agent), content: '正常输出不应隐藏' },
        ], run: { ...snapshot.run, execution: { chainActive: false, codexSession: null, claudeSession: null, turns: [{ promptId: '1', agent, status: 'completed', detail: '', interactions: [{ id: 'q', title, detail: '不应残留的请求', kind: 'mcp_confirmation', status: 'answered', decision, delivery: 'delivered', approval: false, questions: [] }] }] } } };
        const original = JSON.stringify(value);
        const html = render(JSON.parse(original));
        expect(html).not.toContain(title);
        expect(html).not.toContain('不应残留的请求');
        expect(html).not.toContain('未保存表单回答');
        expect(html).toContain('正常输出不应隐藏');
        expect(JSON.stringify(value)).toBe(original);
      }
    }
  });
  it('重试立即替换本卡旧履历，保留其他卡片并在刷新后只展示新输出', () => {
    for (const agent of ['codex', 'claude'] as const) {
      const other = agent === 'codex' ? 'claude' : 'codex';
      const value: RunSnapshot = { ...snapshot, events: [
        { ...event('1', other, 'prompt'), content: '另一卡片原始提示词' },
        { ...event('2', other), content: '另一卡片输出' },
        { ...event('3', agent, 'prompt'), content: '待重试的原始提示词' },
        { ...event('4', agent), content: '不再显示的旧回复' },
        { ...event('5', agent, 'tool_summary'), content: '不再显示的旧工具记录' },
      ], edges: [] };
      expect(render(value)).toContain('不再显示的旧回复');
      // No new reply is needed: the accepted retry prompt clears the card.
      value.events.push({ ...event('6', agent, 'prompt'), content: '待重试的原始提示词' });
      value.edges.push({ id: 'retry', runId: 'r', source: '3', target: '6', edgeType: 'recovery' });
      for (const content of ['', '本次重试的新输出']) {
        if (content) value.events.push({ ...event('7', agent), content });
        const html = render(value);
        expect(html.match(/class="conversation-card"/g)).toHaveLength(2);
        expect(html).toContain('conversation-content-3');
        expect(html).toContain('另一卡片输出');
        expect(html).toContain('待重试的原始提示词');
        expect(html).not.toContain('不再显示的旧');
        if (content) expect(html).toContain(content);
      }
      expect(value.events).toHaveLength(7);
    }
  });
  it('only the final card can retry and repeated attempts stay in that card', () => {
    const value: RunSnapshot = { ...snapshot, events: [event('1', 'codex', 'prompt'), event('2', 'claude', 'prompt')], edges: [], run: { ...snapshot.run, roles: { ...snapshot.run.roles }, execution: { chainActive: false, codexSession: null, claudeSession: null, turns: [
      { promptId: '1', agent: 'codex', logicalRole: 'implementer', roleRevision: 2, status: 'completed', detail: '' },
      { promptId: '2', agent: 'claude', logicalRole: 'analyst', roleRevision: 2, status: 'completed', detail: '' },
    ] } } };
    const markup = () => renderToStaticMarkup(<AgentWorkspace snapshot={value} enabled onSend={async () => true} onStop={async () => {}} onRestart={async () => true} />);
    expect(markup().match(/class="conversation-retry"/g)).toHaveLength(1);
    expect(markup()).toContain('</article><div class="conversation-footer"><button');
    expect(markup()).toContain('aria-hidden="true" focusable="false"><path');
    expect(markup()).not.toContain('>重试</button>');
    expect(markup()).not.toContain('aria-label="重试 Codex 当前会话"');
    expect(markup()).not.toMatch(/class="conversation-retry"[^>]*disabled/);
    const original = conversations(value.events)[0];
    expect(canRetryConversation(value, original)).toBe(false);
    expect(canRetryConversation(value, conversations(value.events)[1])).toBe(true);
    value.run.roles.revision = 3;
    expect(markup().match(/class="conversation-retry"[^>]*disabled/g)).toHaveLength(1);
    value.run.roles.revision = 2;
    value.run.execution!.chainActive = true;
    value.events.push(event('3', 'claude', 'prompt'), event('4', 'claude'));
    value.edges.push({ id: 'retry', runId: 'r', source: '2', target: '3', edgeType: 'recovery' });
    value.run.execution!.turns.push({ promptId: '3', agent: 'claude', status: 'running', detail: '' });
    expect(markup().match(/class="conversation-card"/g)).toHaveLength(2);
    expect(markup().match(/class="conversation-retry"[^>]*disabled/g)).toHaveLength(1);
    expect(markup()).toContain('执行中');
    expect(markup()).toContain('conversation-content-1');
    value.run.execution!.chainActive = false;
    value.run.execution!.turns.at(-1)!.status = 'completed';
    expect(markup()).not.toMatch(/class="conversation-retry"[^>]*disabled/);
    value.events.push(event('5', 'codex', 'prompt'));
    value.run.execution!.turns.push({ promptId: '5', agent: 'codex', logicalRole: 'implementer', roleRevision: 2, status: 'failed', detail: '' });
    expect(markup()).toContain('aria-label="重试 Codex 当前会话"');
    expect(markup()).not.toContain('aria-label="重试 Claude 当前会话"');
    expect(markup().match(/class="conversation-retry"/g)).toHaveLength(1);
    // Composer recovery must not bypass the final-card restriction either.
    value.events.push(event('6', 'claude', 'prompt'));
    value.run.execution!.turns.push({ promptId: '6', agent: 'claude', logicalRole: 'analyst', roleRevision: 2, status: 'completed', detail: '' });
    expect(markup()).not.toContain('仅重启此侧并重试 · Codex');
  });
  it('保留策略拒绝详情并提供单侧重试，运行中禁用重试', () => {
    for (const chainActive of [false, true]) {
      const value: RunSnapshot = { ...snapshot, events: [event('10', 'codex', 'prompt')], edges: [], run: { ...snapshot.run, state: 'paused', execution: { chainActive, codexSession: 'old', claudeSession: 'other', turns: [{ promptId: '10', agent: 'codex', roleRevision: 2, status: 'failed', detail: '受阻', interactions: [{ id: 'blocked', title: 'Codex 权限边界已拒绝', detail: 'cat ../outside', approval: true, questions: [], status: 'blocked' }] }] } } };
      const html = renderToStaticMarkup(<AgentWorkspace snapshot={value} enabled onSend={async () => true} onStop={async () => {}} onRestart={async () => true} />);
      expect(html).toContain('Codex 权限边界已拒绝');
      expect(html).toContain('cat ../outside');
      expect(html).toContain('另一侧会话、待查问题和历史保留');
      expect(html).toContain(`${chainActive ? 'disabled=""' : ''}>仅重启此侧并重试 · Codex`);
      expect(html).not.toContain('>允许本次请求</button>');
    }
  });
  it('每张任务卡片都有当前 agent 的继续执行入口，包括已移交和无回复的任务', () => {
    const html = render(snapshot);
    expect(html.match(/aria-label="继续执行 (Codex|Claude) 当前任务"/g)).toHaveLength(3);
    expect(html.match(/aria-label="给 Codex 的最新指示"/g)).toHaveLength(2);
    expect(html.match(/aria-label="给 Claude 的最新指示"/g)).toHaveLength(1);
    expect(html.match(/disabled="">继续执行 · (Codex|Claude)/g)).toHaveLength(3);
    expect(html).toContain('输入最新指示，继续当前任务（不移交）');
  });
  it('在交接按钮旁显示可选补充要求输入框', () => {
    const html = render({ ...snapshot, edges: [] });
    expect(html).toContain('class="handoff-controls"');
    expect(html).toContain('aria-label="交给 Claude 的补充要求"');
    expect(html).toContain('aria-label="交给 Codex 的补充要求"');
    expect(html).toContain('placeholder="补充要求（可选）"');
  });
  it('保留执行时角色、技能来源和查证交接身份，不用当前映射重写历史', () => {
    const prompt = { ...event('10', 'codex', 'prompt'), logicalRole: 'analyst' as const, provider: 'codex' as const, roleRevision: 1 };
    const value: RunSnapshot = { ...snapshot, events: [prompt], edges: [], run: { ...snapshot.run, execution: { chainActive: false, codexSession: null, claudeSession: null, turns: [{ promptId: '10', agent: 'codex', logicalRole: 'analyst', provider: 'codex', roleRevision: 1, status: 'completed', detail: '', roleSkill: { path: 'D:/test/.rule/analyst/SKILL.md', content: '原始技能' } }] } } };
    const html = render(value);
    expect(html).toContain('分析方 · 角色版本 1');
    expect(html).toContain('D:/test/.rule/analyst/SKILL.md');
    expect(html).toContain('原始技能');
    expect(historicalRole()).toBe('历史角色未记录');
    expect(handoffs({ ...snapshot, edges: [{ ...snapshot.edges[0], edgeType: 'verification' }] })).toHaveLength(1);
  });
  it('does not leak read bodies through the card or failed-task progress', () => {
    const prompt = event('10', 'codex', 'prompt');
    const start = { ...event('11', 'codex', 'stdout'), content: JSON.stringify({ method: 'item/started', params: { item: { id: 'read', type: 'commandExecution', command: 'cat notes.txt' } } }) };
    const operation = { ...event('12', 'codex', 'tool_summary'), content: '开始 · 执行命令 · cat notes.txt', parentId: start.id };
    const raw = { ...event('13', 'codex', 'stdout'), content: JSON.stringify({ method: 'item/commandExecution/outputDelta', params: { itemId: 'read', delta: '# 不应显示的资料正文' } }) };
    const body = { ...event('14', 'codex', 'tool_summary'), content: '# 不应显示的资料正文', parentId: raw.id };
    for (const status of ['running', 'failed', 'completed'] as const) {
      const value: RunSnapshot = { ...snapshot, events: [prompt, start, operation, raw, body], edges: [], run: { ...snapshot.run, execution: { chainActive: status === 'running', codexSession: null, claudeSession: null, turns: [{ promptId: prompt.id, agent: 'codex', status, detail: '' }] } } };
      const html = render(value);
      expect(html).toContain('cat notes.txt');
      expect(html).not.toContain('不应显示的资料正文');
    }
  });
  it('消息接收完成后立即更新 Markdown 状态，不等待整个任务结束', () => {
    const text = '# 报告\n' + '正文'.repeat(650);
    const prompt = event('10', 'codex', 'prompt');
    const raw = { ...event('11', 'codex', 'stdout'), content: JSON.stringify({ type: 'item.completed', item: { type: 'agent_message', id: 'answer' } }) };
    const message = { ...event('12', 'codex'), content: text, parentId: '11' };
    const value: RunSnapshot = { ...snapshot, events: [prompt, raw, message], edges: [], run: { ...snapshot.run, execution: { chainActive: true, codexSession: null, claudeSession: null, turns: [{ promptId: '10', agent: 'codex', status: 'running', detail: '' }] } } };
    expect(render(value)).toContain('内容加载完成');
    expect(render(value)).not.toContain('Markdown加载中');
  });
  it('keeps stopping available while unrelated controls are busy', () => {
    const value: RunSnapshot = { ...snapshot, run: { ...snapshot.run, execution: { chainActive: true, codexSession: null, claudeSession: null, turns: [] } } };
    const html = renderToStaticMarkup(<AgentWorkspace snapshot={value} enabled={false} onSend={async () => false} onStop={async () => {}} />);
    expect(html).toContain('<button type="button">停止当前执行</button>');
    expect(html).toMatch(/disabled="">发送给 Codex/);
  });
  it('offers recovery directly in the composer after cleanup, without replaying a task', () => {
    for (const state of ['paused', 'interrupted'] as const) {
      for (const chainActive of [true, false]) {
        const value: RunSnapshot = { ...snapshot, run: { ...snapshot.run, state, execution: { chainActive, codexSession: 'retained', claudeSession: null, turns: [] } } };
        const html = renderToStaticMarkup(<AgentWorkspace snapshot={value} enabled onSend={async () => true} onStop={async () => {}} onRecover={async () => true} />);
        expect(html).toContain('确认检查点并恢复输入');
        expect(html).toContain('不会重放旧任务');
        expect(html).toMatch(/disabled="">发送给 Codex/);
        expect(html.includes('disabled="">确认检查点并恢复输入')).toBe(chainActive);
      }
    }
    expect(render(snapshot)).not.toContain('确认检查点并恢复输入');
  });
  it('defaults automatic handoff off and shows persisted turn status', () => {
    const value: RunSnapshot = { ...snapshot, run: { ...snapshot.run, execution: { chainActive: false, codexSession: 'session', claudeSession: null, turns: [{ promptId: '2', agent: 'claude', status: 'failed', detail: '失败' }] } } };
    const html = render(value);
    expect(html).toContain('已保存此侧会话');
    expect(html).toContain('最多 4 次');
    expect(html).not.toContain('checked=""');
    expect(html).toContain('<span class="muted">失败</span>');
    expect(deliveryStatus(value, '2')).toBe('失败');
    expect(deliveryStatus(value, '4')).toBe('指令已接受');
  });
  it('blocks sending during inter-turn gaps while retaining stop control', () => {
    const html = render({ ...snapshot, run: { ...snapshot.run, execution: { chainActive: true, codexSession: null, claudeSession: null, turns: [] } } });
    expect(html).toMatch(/disabled="">发送给 Codex/);
    expect(html).toContain('停止当前执行');
  });
  it('shows alternating conversation cards without arrows or split panels', () => {
    const html = render(snapshot);
    expect(handoffs(snapshot)).toHaveLength(2);
    expect(html.match(/class="conversation-row (codex|claude)"/g)).toEqual([
      'class="conversation-row codex"', 'class="conversation-row claude"', 'class="conversation-row codex"',
    ]);
    expect(html).not.toContain('handoff-arrow');
    expect(html).not.toContain('agent-columns');
    expect(html).not.toContain('⟶'); expect(html).not.toContain('⟵');
    expect(html.match(/任务时间：/g)).toHaveLength(3);
    expect(html.match(/aria-expanded="false"/g)).toHaveLength(2);
    expect(html.match(/aria-expanded="true"/g)).toHaveLength(1);
    expect(html).toContain('收起内容');
    expect(html).toContain('展开内容');
    expect(html).not.toContain('因果链');
    expect(html).toContain('&lt;原文&gt;');
  });
  it('keeps identity-based selection after role swaps and no fake links in empty state', () => {
    expect(render(snapshot)).toContain('<option value="codex" selected="">Codex</option>');
    expect(render(snapshot)).toContain('<option value="claude">Claude</option>');
    expect(render(snapshot)).toContain('label for="prompt-recipient"');
    const html = render(null);
    expect(html).toContain('发送给 Codex');
    expect(html).not.toContain('handoff-arrow');
    expect(html).toMatch(/disabled="">发送给 Codex/);
  });
  it('omits same-agent and dangling edges', () => {
    expect(handoffs({ ...snapshot, edges: [
      { id: 'x', runId: 'r', source: '1', target: '4', edgeType: 'handoff' },
      { id: 'y', runId: 'r', source: 'missing', target: '4', edgeType: 'handoff' },
    ] })).toEqual([]);
  });
  it('groups prompts and output in persisted sequence order without mutating the snapshot', () => {
    const events = [event('6', 'codex', 'prompt'), event('4', 'claude', 'prompt'), event('1', 'codex', 'prompt'), event('3', null, 'system'), event('2', 'codex', 'stdout'), event('5', 'claude'), event('7', 'codex')];
    const groups = conversations(events);
    expect(groups.map(group => group.events.map(item => item.id))).toEqual([['1', '2'], ['4', '5'], ['6', '7']]);
    expect(groups.map(group => group.promptId)).toEqual(['1', '4', '6']);
    expect(events[0].id).toBe('6');
  });
  it('keeps separate turns from the same agent and legacy output visible', () => {
    const groups = conversations([event('1', 'codex'), event('2', 'codex', 'prompt'), event('3', 'codex', 'prompt')]);
    expect(groups).toHaveLength(3);
    expect(groups[0].promptId).toBeUndefined();
    expect(conversations([event('1', null)])).toEqual([]);
  });
});