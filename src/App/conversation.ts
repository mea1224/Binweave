import type { AgentEvent, AgentId, CausalEdge } from '../types';

export interface Conversation {
  id: string;
  agent: AgentId;
  createdAt: string;
  promptId?: string;
  events: AgentEvent[];
}

// Sequence is the persisted execution order; timestamps may share a second.
// Only explicit recovery edges replace an existing card with its latest attempt.
export function conversations(events: AgentEvent[], edges: CausalEdge[] = []): Conversation[] {
  return conversationProjection()([...events].sort((a, b) => a.sequence - b.sequence), edges);
}

// Preserve untouched conversation objects. Appends only copy the active card;
// a refresh/history replacement rebuilds the projection from its new source.
export function conversationProjection() {
  let source: AgentEvent[] = [];
  let result: Conversation[] = [];
  let routes = new Map<string, number>();
  let active = -1;
  let recoveryKey = '';
  return (events: AgentEvent[], edges: CausalEdge[] = []) => {
    const recoveries = edges.filter(edge => edge.edgeType === 'recovery');
    const key = JSON.stringify(recoveries.map(edge => [edge.source, edge.target]));
    if (events === source && key === recoveryKey) return result;
    // A recovery edge may arrive with a full snapshot or an incremental update.
    if (key !== recoveryKey || events.length < source.length || (source.length && (events[0] !== source[0] || events[source.length - 1] !== source.at(-1)))) {
      result = []; source = []; routes = new Map(); active = -1;
    }
    const parents = new Map(recoveries.map(edge => [edge.target, edge.source]));
    const next = [...result];
    const copied = new Set<number>();
    for (const event of events.slice(source.length)) {
      if (!event.agent) continue;
      const retry = event.kind === 'prompt' ? routes.get(parents.get(event.id) ?? '') : undefined;
      if (retry !== undefined && next[retry].agent === event.agent) active = retry;
      else if (active < 0 || next[active].agent !== event.agent || event.kind === 'prompt') {
        active = next.length;
        next.push({ id: event.id, agent: event.agent, createdAt: event.createdAt, events: [] });
        copied.add(active);
      }
      if (!copied.has(active)) {
        next[active] = { ...next[active], events: [...next[active].events] };
        copied.add(active);
      }
      const current = next[active];
      if (event.kind === 'prompt') {
        current.promptId = event.id;
        current.createdAt = event.createdAt;
        current.events = [];
      }
      current.events.push(event);
      routes.set(event.id, active);
    }
    result = next;
    source = events;
    recoveryKey = key;
    return result;
  };
}