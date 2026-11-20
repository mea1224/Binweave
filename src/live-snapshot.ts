import type { RunSnapshot } from './types';

// The active workspace subscribes here; token batches do not invalidate App,
// navigation, diagnostics or recovery panels.
export function liveSnapshot() {
  let snapshot: RunSnapshot | null = null;
  const listeners = new Set<() => void>();
  return {
    get: () => snapshot,
    subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener); }; },
    set(next: RunSnapshot | null) {
      if (next === snapshot) return;
      snapshot = next;
      listeners.forEach(listener => listener());
    },
  };
}

export function controlsChanged(previous: RunSnapshot | null, next: RunSnapshot | null) {
  return previous?.run.id !== next?.run.id || previous?.run.state !== next?.run.state
    || JSON.stringify(previous?.run.execution) !== JSON.stringify(next?.run.execution)
    || previous?.artifacts.length !== next?.artifacts.length;
}