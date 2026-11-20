import { describe, expect, it } from 'vitest';
import { applyRunUpdate, frameQueue, type RunUpdate } from './stream-updates';
import type { RunSnapshot } from './types';

const base: RunSnapshot = {
  run: { id: 'r', title: '', workspace: '', createdAt: '', updatedAt: '2026-11-20T10:00:00Z', state: 'ready', roles: { analyst: 'codex', implementer: 'claude', revision: 1 } },
  checkpoint: { runId: 'r', state: 'ready', activeAgent: null, nextStep: '', lastEventId: null, updatedAt: '', roleRevision: 1, interruptedState: null }, events: [], edges: [], artifacts: [],
};
const update: RunUpdate = { ...base, baseEventCount: 0, events: [{ id: 'e', runId: 'r', sequence: 1, createdAt: '', agent: 'codex', kind: 'message', content: '片段', parentId: null }] };

describe('incremental stream updates', () => {
  it('merges overlapping updates without duplicating events or replacing old event references', () => {
    const first = applyRunUpdate(base, update)!;
    const repeated = applyRunUpdate(first, update)!;
    expect(repeated.events).toHaveLength(1);
    expect(repeated.events[0]).toBe(first.events[0]);
    expect(applyRunUpdate(first, { ...update, baseEventCount: 1, events: [{ ...update.events[0], id: 'e2', sequence: 2 }] })?.events).toHaveLength(2);
  });
  it('rejects gaps, old histories and other workspaces', () => {
    expect(applyRunUpdate(base, { ...update, baseEventCount: 4 })).toBe(base);
    expect(applyRunUpdate(base, { ...update, run: { ...base.run, id: 'other' } })).toBe(base);
    const cleared = { ...base, run: { ...base.run, execution: { chainActive: false, codexSession: null, claudeSession: null, turns: [], historyRevision: 1 } } };
    expect(applyRunUpdate(cleared, update)).toBe(cleared);
  });
  it('renders a burst once per frame without losing the final fragment and cancels on teardown', () => {
    let callback: FrameRequestCallback = () => {};
    let scheduled = 0;
    let cancelled = 0;
    const rendered: number[][] = [];
    const queue = frameQueue<number>(items => rendered.push(items), cb => { callback = cb; return ++scheduled; }, () => cancelled++);
    for (let i = 0; i < 100; i++) queue.push(i);
    expect(scheduled).toBe(1);
    callback(0);
    expect(rendered[0]).toHaveLength(100);
    expect(rendered[0].at(-1)).toBe(99);
    queue.push(100); queue.dispose();
    expect(cancelled).toBe(1);
    expect(rendered).toHaveLength(1);
  });
});