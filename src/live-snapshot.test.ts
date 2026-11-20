import { describe, expect, it, vi } from 'vitest';
import { controlsChanged, liveSnapshot } from './live-snapshot';
import type { RunSnapshot } from './types';

describe('workspace-only live snapshots', () => {
  it('notifies only subscribers and detaches cleanly', () => {
    const live = liveSnapshot();
    const listener = vi.fn();
    const off = live.subscribe(listener);
    live.set(null);
    expect(listener).not.toHaveBeenCalled();
    const snapshot = { run: { id: 'r' }, events: [] } as unknown as RunSnapshot;
    live.set(snapshot);
    expect(live.get()).toBe(snapshot);
    expect(listener).toHaveBeenCalledTimes(1);
    off(); live.set(null);
    expect(listener).toHaveBeenCalledTimes(1);
  });
  it('does not invalidate App controls for text, but does for execution changes', () => {
    const before = { run: { id: 'r', state: 'analyzing', execution: { chainActive: true, turns: [] } }, events: [], artifacts: [] } as unknown as RunSnapshot;
    expect(controlsChanged(before, { ...before, run: { ...before.run, updatedAt: 'later' } })).toBe(false);
    expect(controlsChanged(before, { ...before, run: { ...before.run, state: 'paused' } })).toBe(true);
    expect(controlsChanged(before, null)).toBe(true);
  });
});