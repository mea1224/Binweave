import type { AgentEvent } from '../types';

const parsed = new WeakMap<AgentEvent, object | null>();
export function protocol<T extends object>(event: AgentEvent): T | undefined {
  if (!parsed.has(event)) {
    try {
      const value: unknown = JSON.parse(event.content);
      parsed.set(event, value !== null && typeof value === 'object' ? value : null);
    } catch { parsed.set(event, null); }
  }
  return (parsed.get(event) ?? undefined) as T | undefined;
}