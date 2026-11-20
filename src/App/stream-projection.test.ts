import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '../types';
import { conversationProjection, conversations } from './conversation';
import { displayEvents, displayProjection } from './display-events';

const event = (id: number, kind: AgentEvent['kind'], content = '', parentId: string | null = null): AgentEvent => ({ id: String(id), runId: 'run', sequence: id, createdAt: '', agent: 'codex', kind, content, parentId });

describe('incremental conversation projections', () => {
  it('replaces only the retried card history across refreshes and repeated attempts', () => {
    const project = conversationProjection();
    const first = [event(1, 'prompt', '原始任务'), event(2, 'message', '旧回复'), { ...event(3, 'prompt', '另一侧'), agent: 'claude' as const }];
    const previous = project(first);
    const edges = [{ id: 'r1', runId: 'run', source: '1', target: '4', edgeType: 'recovery' as const }];
    const retry = [...first, event(4, 'prompt', '原始任务'), event(5, 'message', '重试回复')];
    const retried = project(retry, edges);
    expect(retried.map(c => c.id)).toEqual(['1', '3']);
    expect(retried[0].promptId).toBe('4');
    expect(retried[0].events.map(e => e.id)).toEqual(['4', '5']);
    expect(retried[1]).toEqual(previous[1]);
    expect(previous[0].events).toHaveLength(2);
    const streamed = [...retry, event(6, 'message', '新输出')];
    const next = project(streamed, edges);
    expect(next[1]).toBe(retried[1]);
    expect(retried[0].events).toHaveLength(2);
    expect(next).toEqual(conversations(streamed, edges));
    const repeated = [...streamed, event(7, 'prompt', '原始任务'), event(8, 'message')];
    edges.push({ id: 'r2', runId: 'run', source: '4', target: '7', edgeType: 'recovery' });
    expect(project(repeated, edges)).toEqual(conversations(repeated.map(e => ({ ...e })), edges));
    expect(project(repeated, edges).map(c => [c.id, c.promptId])).toEqual([['1', '7'], ['3', '3']]);
    expect(project(repeated, edges)[0].events.map(e => e.id)).toEqual(['7', '8']);
    expect(project(repeated, edges)[1]).toEqual(previous[1]);
  });
  it('retains completed conversation references and never mutates prior renders', () => {
    const project = conversationProjection();
    const first = [event(1, 'prompt'), event(2, 'message'), event(3, 'prompt')];
    const previous = project(first);
    const events = [...first, event(4, 'message', 'delta')];
    const next = project(events);
    expect(next).toEqual(conversations(events));
    expect(next[0]).toBe(previous[0]);
    expect(next[1]).not.toBe(previous[1]);
    expect(previous[1].events).toHaveLength(1);
    expect(project([])).toEqual([]);
    expect(project(first.map(e => ({ ...e })))).toEqual(conversations(first));
  });
  it('joins only new deltas, isolates stream IDs, and reconciles refresh and final text', () => {
    const project = displayProjection();
    const history = event(1, 'message', 'history');
    let events = [history];
    let last = project(events);
    for (let i = 0; i < 100; i++) {
      const id = 2 + i * 2;
      events = [...events, event(id, 'stdout', JSON.stringify({ method: 'item/agentMessage/delta', params: { itemId: `m${i % 2}`, delta: '中' } })), event(id + 1, 'message', '中', String(id))];
      const next = project(events);
      expect(next).toEqual(displayEvents(events));
      expect(next[0]).toBe(last[0]);
      expect(last.slice(1).reduce((sum, e) => sum + e.content.length, 0)).toBe(i);
      last = next;
    }
    expect(last.slice(1).map(e => e.content)).toEqual(['中'.repeat(50), '中'.repeat(50)]);
    const refreshed = events.map(e => ({ ...e }));
    expect(project(refreshed)).toEqual(displayEvents(refreshed));
    expect(project([])).toEqual([]);
  });
  it('retains Claude stream context between batches and replaces the final message', () => {
    const project = displayProjection();
    const raw = (id: number, value: unknown) => ({ ...event(id, 'stdout', JSON.stringify(value)), agent: 'claude' as const });
    const first = [raw(1, { type: 'stream_event', event: { type: 'message_start', message: { id: 'm' } } }), raw(2, { type: 'stream_event', event: { type: 'content_block_delta', delta: { type: 'text_delta' } } }), { ...event(3, 'message', '早到的文字', '2'), agent: 'claude' as const }];
    const initial = project(first);
    expect(initial[0].content).toBe('早到的文字');
    const final = [...first, raw(4, { type: 'assistant', message: { id: 'm', content: [{ type: 'text', text: '完整文字' }] } }), { ...event(5, 'message', '完整文字', '4'), agent: 'claude' as const }];
    expect(project(final)).toEqual(displayEvents(final));
    expect(initial[0].content).toBe('早到的文字');
  });
});