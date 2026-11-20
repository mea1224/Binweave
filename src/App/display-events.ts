import type { AgentEvent } from '../types';
import { markdownFileLabel, readActivity, readToolLabel } from './read-activity';
import { protocol } from './protocol';

interface WireEvent {
  method?: string;
  type?: string;
  params?: { delta?: string; itemId?: string; summaryIndex?: number; contentIndex?: number };
  item?: { type?: string; id?: string };
  message?: { id?: string; content?: { type: string; text?: string; name?: string; input?: { command?: string; file_path?: string; path?: string; url?: string } }[] };
  event?: { type?: string; index?: number; message?: { id?: string }; delta?: { type?: string } };
}

// Keep the persisted audit trail intact; join deltas only in the card projection.
export function displayEvents(events: AgentEvent[]): (AgentEvent & { complete?: boolean })[] {
  return displayProjection()(events);
}

export function displayProjection() {
  let result: (AgentEvent & { complete?: boolean })[] = [];
  const raw = new Map<string, WireEvent>();
  const streams = new Map<string, number>();
  let claudeMessage = '';
  let source: AgentEvent[] = [];
  let readState = readActivity([]).state;
  return (events: AgentEvent[]) => {
  if (events === source) return result;
  if (events.length < source.length || (source.length && (events[0] !== source[0] || events[source.length - 1] !== source.at(-1)))) {
    result = []; raw.clear(); streams.clear(); claudeMessage = ''; source = []; readState = readActivity([]).state;
  }
  const added = events.slice(source.length);
  const reads = readActivity(added, readState);
  result = [...result];
  for (const event of added) {
    if (event.kind === 'stdout') {
      try {
        const value = protocol<WireEvent>(event);
        if (value && typeof value === 'object') {
          raw.set(event.id, value);
          if (value.type === 'stream_event' && value.event?.type === 'message_start') claudeMessage = value.event.message?.id ?? event.id;
          if (value.type === 'stream_event' && value.event?.type === 'message_stop') {
            const index = streams.get(`claude:${claudeMessage}:text_delta`);
            if (index !== undefined) result[index] = { ...result[index], complete: true };
          }
          continue; // Protocol envelopes remain available in the audit view.
        }
      } catch { /* Non-protocol stdout is still readable in the card. */ }
    }
    const wire = event.parentId ? raw.get(event.parentId) : undefined;
    const replacement = reads.replacement(event);
    if (replacement !== undefined) {
      if (replacement) result.push({ ...event, kind: 'tool_summary', content: replacement });
      continue;
    }
    if (wire?.type === 'assistant' && wire.message?.id && event.kind === 'tool_summary') {
      const text = wire.message.content?.filter(b => b.type === 'text').map(b => b.text ?? '').join('\n');
      const index = streams.get(`claude:${wire.message.id}:text_delta`);
      if (index !== undefined && text) result[index] = { ...result[index], content: text, complete: true };
      const tools = wire.message.content?.filter(b => b.type === 'tool_use').map(b => markdownFileLabel(b) ?? readToolLabel(b) ?? b.name).filter(Boolean).join('\n');
      result.push({ ...event, content: [index === undefined ? text : '', tools].filter(Boolean).join('\n') });
      continue;
    }
    let key: string | undefined;
    let append = false;
    if (wire?.method && typeof wire.params?.delta === 'string') {
      key = `codex:${wire.params.itemId}:${wire.method}${wire.params.summaryIndex ?? wire.params.contentIndex ?? ''}`;
      append = true;
    } else if (wire?.type === 'item.completed' && wire.item?.type === 'agent_message' && wire.item.id) {
      key = `codex:${wire.item.id}:item/agentMessage/delta`;
    } else if (wire?.type === 'stream_event' && wire.event?.type === 'content_block_delta') {
      key = `claude:${claudeMessage}:${wire.event.delta?.type}`;
      append = true;
    } else if (wire?.type === 'assistant' && wire.message?.id && event.kind === 'message') {
      key = `claude:${wire.message.id}:text_delta`;
    }
    if (wire?.type === 'result' && event.kind === 'message') {
      const last = [...result].reverse().find(e => e.kind === 'message');
      if (last?.content === event.content) {
        // Retain the final persisted ID for handoff links.
        result[result.indexOf(last)] = { ...event, complete: true };
        continue;
      }
    }
    const index = key ? streams.get(key) : undefined;
    const complete = wire?.type === 'assistant' || wire?.type === 'result' || wire?.type === 'item.completed';
    if (index !== undefined) {
      result[index] = { ...event, content: append ? result[index].content + event.content : event.content, complete };
    } else {
      if (key) streams.set(key, result.length);
      result.push({ ...event, complete });
    }
  }
  source = events;
  return result;
  };
}