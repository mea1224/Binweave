import type { AgentEvent } from '../types';
import { protocol } from './protocol';

interface Tool {
  id?: string; type?: string; name?: string; tool?: string; command?: string;
  input?: { command?: string; file_path?: string; path?: string; url?: string };
  changes?: { path?: string }[];
}

interface ReadWire {
  type?: string; method?: string;
  item?: Tool;
  params?: { item?: Tool; itemId?: string };
  message?: { content?: (Tool & { tool_use_id?: string })[] };
  event?: { content_block?: Tool };
}

// Match executable read operations, not file extensions or words in an answer.
export function isReadCommand(command: string): boolean {
  const wrapper = command.trim().match(/^(?:\S*[\\/])?(?:powershell(?:\.exe)?|pwsh(?:\.exe)?|bash|sh)\s+(?:-\S+\s+)*-(?:Command|c|lc)\s+(["'])([\s\S]*)\1$/i);
  const stripped = wrapper?.[2] ?? command;
  // Quoted arguments are data, not commands. Reject mixed read/build scripts
  // rather than hiding their unrelated diagnostics along with the document.
  const masked = stripped.replace(/"(?:`.|\\.|[^"\\])*"|'(?:''|[^'])*'/g, 'argument');
  const segments = masked.split(/&&|\|\||[;|\r\n]/).map(s => s.trim()).filter(Boolean);
  const read = /^(?:Get-Content|gc|cat|head|tail|less|more|type|Select-String|rg|grep|sed|curl|wget)(?:\.exe)?\s+\S/i;
  const format = /^(?:Select-Object|Format-Table|Format-List|Out-String)(?:\s|$)/i;
  return !!segments.length && read.test(segments[0]) && segments.every(segment => read.test(segment) || format.test(segment));
}

export function readToolLabel(tool: Tool): string | undefined {
  const name = tool.name ?? tool.tool ?? '';
  const command = tool.command ?? tool.input?.command;
  if (command && isReadCommand(command)) return `读取资料 · ${command}`;
  if (/(?:^|[_.:/-])(?:read|read_file|read_files|read_text_file|read_multiple_files|read_resource|fetch|webfetch|fetch_web_content|search_codebase)$/i.test(name)) {
    const path = tool.input?.file_path ?? tool.input?.path ?? tool.input?.url;
    return `读取资料 · ${name}${path ? ` · ${path}` : ''}`;
  }
  return undefined;
}

export function readActivity(events: AgentEvent[], state = { raw: new Map<string, ReadWire>(), tools: new Map<string, string>() }) {
  const { raw, tools } = state;
  for (const event of events) {
    if (event.kind !== 'stdout') continue;
    try {
      const wire = protocol<ReadWire>(event);
      if (!wire || typeof wire !== 'object') continue;
      raw.set(event.id, wire);
      const blocks = Array.isArray(wire.message?.content) ? wire.message.content : [];
      const candidates = [wire.item, wire.params?.item, wire.event?.content_block, ...blocks];
      for (const tool of candidates) {
        if (!tool?.id) continue;
        const label = readToolLabel(tool);
        if (label) tools.set(`${event.agent}:${tool.id}`, label);
      }
    } catch { /* Plain output has no reliable tool attribution. */ }
  }
  return {
    state,
    replacement(event: AgentEvent): string | undefined {
      if (!['tool_summary', 'message'].includes(event.kind) || !event.parentId) return undefined;
      const wire = raw.get(event.parentId);
      if (!wire) return undefined;
      const item = wire.params?.item ?? wire.item ?? wire.event?.content_block;
      const fileLabel = item && markdownFileLabel(item);
      // Keep the original operation summary (paths, exit status and diagnostics).
      if (fileLabel) return `${fileLabel}\n${event.content}`;
      if (wire.method === 'item/commandExecution/outputDelta' || wire.method === 'item/mcpToolCall/outputDelta') {
        const label = tools.get(`${event.agent}:${wire.params?.itemId}`);
        if (label) return ''; // The start/completion event retains the operation.
      }
      const blocks = Array.isArray(wire.message?.content) ? wire.message.content : [];
      const results = blocks.filter(block => block?.type === 'tool_result');
      if (results.length && results.length === blocks.length && results.every(block => tools.has(`${event.agent}:${block.tool_use_id}`))) {
        return results.map(block => `完成 · ${tools.get(`${event.agent}:${block.tool_use_id}`)}`).join('\n');
      }
      return undefined;
    },
  };
}

export function markdownFileLabel(tool: Tool): string | undefined {
  const name = tool.name ?? tool.tool ?? '';
  const command = tool.command ?? tool.input?.command;
  const read = !!readToolLabel(tool);
  const write = /^(?:Write|Edit|MultiEdit|write_file|edit_file)$/i.test(name) || ['fileChange', 'file_change'].includes(tool.type ?? '');
  if (!read && !write) return undefined;
  const paths = [tool.input?.file_path, tool.input?.path, ...(tool.changes ?? []).map(change => change.path)]
    .filter((path): path is string => !!path && /\.(?:md|markdown)$/i.test(path));
  if (read && command) {
    for (const match of command.matchAll(/"([^"\r\n]+\.(?:md|markdown))"|'([^'\r\n]+\.(?:md|markdown))'|([^\s"'<>|;]+\.(?:md|markdown))(?=\s|$)/gi)) {
      paths.push(match[1] ?? match[2] ?? match[3]);
    }
  }
  const files = [...new Set(paths.map(path => path.split(/[\\/]/).at(-1)!))];
  return files.length ? `${read ? '读入' : '输出'} · ${files.join('、')}` : undefined;
}