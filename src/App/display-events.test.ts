import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '../types';
import { displayEvents } from './display-events';

function fixture(agent: 'codex' | 'claude') {
  const events: AgentEvent[] = [];
  const add = (wire: object, text?: string, kind: AgentEvent['kind'] = 'message') => {
    const raw: AgentEvent = { id: String(events.length), runId: 'r', sequence: events.length, createdAt: '', agent, kind: 'stdout', content: JSON.stringify(wire), parentId: null };
    events.push(raw);
    if (text !== undefined) events.push({ ...raw, id: String(events.length), sequence: events.length, kind, content: text, parentId: raw.id });
  };
  return { events, add };
}

describe('streamed card output', () => {
  it('renders Codex before completion, joins deltas, and reconciles final text and source ID', () => {
    const { events, add } = fixture('codex');
    add({ method: 'item/agentMessage/delta', params: { itemId: 'a', delta: '你好' } }, '你好');
    expect(displayEvents(events).map(e => e.content)).toEqual(['你好']);
    expect(displayEvents(events)[0].complete).toBe(false);
    add({ method: 'item/agentMessage/delta', params: { itemId: 'a', delta: ' world' } }, ' world');
    expect(displayEvents(events).map(e => e.content)).toEqual(['你好 world']);
    add({ type: 'item.completed', item: { id: 'a', type: 'agent_message' } }, '你好 world');
    expect(displayEvents(events)).toHaveLength(1);
    expect(displayEvents(events)[0].id).toBe(events.at(-1)?.id);
    expect(displayEvents(events)[0].complete).toBe(true);
    expect(events).toHaveLength(6);
  });

  it('streams Claude messages separately and does not duplicate assistant/result envelopes', () => {
    const { events, add } = fixture('claude');
    for (const id of ['a', 'b']) {
      add({ type: 'stream_event', event: { type: 'message_start', message: { id } } });
      for (const text of ['第一段', '\n第二段']) add({ type: 'stream_event', event: { type: 'content_block_delta', delta: { type: 'text_delta' } } }, text);
      expect(displayEvents(events).at(-1)?.content).toBe('第一段\n第二段');
      expect(displayEvents(events).at(-1)?.complete).toBe(false);
      add({ type: 'stream_event', event: { type: 'message_stop' } });
      expect(displayEvents(events).at(-1)?.complete).toBe(true);
      add({ type: 'assistant', message: { id } }, '第一段\n第二段');
    }
    add({ type: 'result' }, '第一段\n第二段');
    expect(displayEvents(events).map(e => e.content)).toEqual(['第一段\n第二段', '第一段\n第二段']);
    expect(displayEvents(events).at(-1)?.id).toBe(events.at(-1)?.id);
  });

  it('keeps tool deltas separate from assistant text and hides protocol JSON only in the card', () => {
    const { events, add } = fixture('codex');
    add({ method: 'item/commandExecution/outputDelta', params: { itemId: 'tool', delta: '输出' } }, '输出', 'tool_summary');
    add({ method: 'item/agentMessage/delta', params: { itemId: 'answer', delta: '答复' } }, '答复');
    expect(displayEvents(events).map(e => [e.kind, e.content])).toEqual([['tool_summary', '输出'], ['message', '答复']]);
    expect(events[0].content).toContain('outputDelta');
  });

  it('hides streamed read bodies while retaining operations, replies and the audit trail', () => {
    const { events, add } = fixture('codex');
    add({ method: 'item/started', params: { item: { id: 'read', type: 'commandExecution', command: 'Get-Content README.md' } } }, '开始 · 执行命令 · Get-Content README.md', 'tool_summary');
    add({ method: 'item/commandExecution/outputDelta', params: { itemId: 'read', delta: '# 私有资料正文' } }, '# 私有资料正文', 'tool_summary');
    expect(displayEvents(events).map(e => e.content)).toEqual(['读入 · README.md\n开始 · 执行命令 · Get-Content README.md']);
    add({ method: 'item/started', params: { item: { id: 'test', type: 'commandExecution', command: 'npm test' } } }, '开始 · 执行命令 · npm test', 'tool_summary');
    add({ method: 'item/commandExecution/outputDelta', params: { itemId: 'test', delta: '测试失败' } }, '测试失败', 'tool_summary');
    add({ method: 'item/agentMessage/delta', params: { itemId: 'answer', delta: '# 分析结论' } }, '# 分析结论');
    add({ method: 'item/completed', params: { item: { id: 'read', type: 'commandExecution', command: 'Get-Content README.md' } } }, '完成 · 执行命令 · Get-Content README.md', 'tool_summary');
    const original = JSON.stringify(events);
    const visible = displayEvents(events).map(e => e.content).join('\n');
    expect(visible).not.toContain('私有资料正文');
    expect(visible).toContain('测试失败');
    expect(visible).toContain('# 分析结论');
    expect(visible).toContain('读入 · README.md');
    expect(visible).toContain('完成 · 执行命令 · Get-Content README.md');
    expect(JSON.stringify(events)).toBe(original);
    expect(original).toContain('私有资料正文');
  });

  it('retains Claude read paths and assistant text without displaying read results', () => {
    const { events, add } = fixture('claude');
    add({ type: 'assistant', message: { id: 'a', content: [{ type: 'text', text: '我先查看资料' }, { type: 'tool_use', id: 'r', name: 'Read', input: { file_path: 'D:/notes.txt' } }] } }, '我先查看资料\nRead', 'tool_summary');
    add({ type: 'user', message: { content: [{ type: 'tool_result', tool_use_id: 'r', content: '资料正文' }] } }, '资料正文', 'tool_summary');
    add({ type: 'assistant', message: { id: 'b' } }, '# 正常 Markdown 回复');
    const visible = displayEvents(events).map(e => e.content).join('\n');
    expect(visible).toContain('我先查看资料');
    expect(visible).toContain('读取资料 · Read · D:/notes.txt');
    expect(visible).not.toContain('资料正文');
    expect(visible).toContain('# 正常 Markdown 回复');
  });

  it('recognizes historical read commands without filtering unattributed output', () => {
    const { events, add } = fixture('codex');
    add({ type: 'item.started', item: { id: 'r', type: 'command_execution', command: 'cat notes.txt' } }, '开始 · cat notes.txt', 'tool_summary');
    add({ method: 'item/commandExecution/outputDelta', params: { itemId: 'r', delta: '正文' } }, '正文', 'tool_summary');
    add({ method: 'item/commandExecution/outputDelta', params: { itemId: 'unknown', delta: '# 未归属输出' } }, '# 未归属输出', 'tool_summary');
    expect(displayEvents(events).map(e => e.content)).toEqual(['开始 · cat notes.txt', '# 未归属输出']);
  });

  it('preserves read stderr and tolerates non-block protocol messages', () => {
    const { events, add } = fixture('codex');
    add({ method: 'item/started', params: { item: { id: 'r', type: 'commandExecution', command: 'cat notes.txt' } } }, '开始 · cat notes.txt', 'tool_summary');
    add({ method: 'item/commandExecution/outputDelta', params: { itemId: 'r', delta: '读取失败' } }, '读取失败', 'stderr');
    add({ type: 'user', message: { content: '普通协议文本' } }, '保留未知摘要', 'tool_summary');
    expect(displayEvents(events).map(e => e.content)).toEqual(['开始 · cat notes.txt', '读取失败', '保留未知摘要']);
  });
});