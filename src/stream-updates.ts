import type { RunSnapshot } from './types';
import { newerSnapshot } from './view-model';

export interface RunUpdate extends RunSnapshot { baseEventCount: number }

export function applyRunUpdate(current: RunSnapshot | null, update: RunUpdate): RunSnapshot | null {
  if (!current || current.run.id !== update.run.id) return current;
  if ((current.run.execution?.historyRevision ?? 0) !== (update.run.execution?.historyRevision ?? 0)) return current;
  if (update.baseEventCount > current.events.length) return current;
  const end = update.baseEventCount + update.events.length;
  if (end < current.events.length || Date.parse(update.run.updatedAt) < Date.parse(current.run.updatedAt)) return current;
  const events = [...current.events, ...update.events.slice(current.events.length - update.baseEventCount)];
  const edgeIds = new Set(current.edges.map(edge => edge.id));
  const artifactIds = new Set(current.artifacts.map(artifact => artifact.id));
  return newerSnapshot(current, { run: update.run, checkpoint: update.checkpoint, events,
    edges: [...current.edges, ...update.edges.filter(edge => !edgeIds.has(edge.id))],
    artifacts: [...current.artifacts, ...update.artifacts.filter(artifact => !artifactIds.has(artifact.id))] });
}

// Keep all deltas, but commit React state only once per display frame.
export function frameQueue<T>(flush: (items: T[]) => void, request: (callback: FrameRequestCallback) => number = requestAnimationFrame, cancel: (id: number) => void = cancelAnimationFrame) {
  let pending: T[] = [];
  let frame: number | undefined;
  return {
    push(item: T) {
      pending.push(item);
      if (frame === undefined) frame = request(() => {
        frame = undefined;
        const items = pending; pending = [];
        flush(items);
      });
    },
    dispose() { if (frame !== undefined) cancel(frame); frame = undefined; pending = []; },
  };
}