import { Fragment, type ReactNode } from 'react';

// Render React nodes, never raw HTML. Unsupported syntax stays readable as text.
function linkTarget(value: string): string | undefined {
  const target = value.trim().replace(/^<|>$/g, '');
  return /^(?:https?:\/\/|mailto:|#)/i.test(target) && !/[\u0000-\u0020]/.test(target) ? target : undefined;
}

export function inlineMarkdown(text: string, depth = 0): ReactNode {
  if (depth > 12) return text;
  const pattern = /\\([\\`*{}\[\]()#+.!_>~-])|(`+)([\s\S]*?)\2(?!`)|!?\[([^\]\n]+)\]\((<[^>\n]+>|[^\s)]+)(?:\s+"[^"\n]*")?\)|\*\*([\s\S]+?)\*\*|__([\s\S]+?)__|~~([\s\S]+?)~~|\*([^*\n]+)\*|(?<!\w)_([^_\n]+)_(?!\w)|<(https?:\/\/[^>\s]+)>/g;
  const nodes: ReactNode[] = [];
  let offset = 0;
  for (const match of text.matchAll(pattern)) {
    nodes.push(text.slice(offset, match.index));
    let node: ReactNode;
    if (match[1]) node = match[1];
    else if (match[2]) node = <code className="inline-code">{match[3]}</code>;
    else if (match[4] || match[11]) {
      const label = match[4] ?? match[11];
      const href = linkTarget(match[5] ?? match[11]);
      node = href ? <a href={href} target="_blank" rel="noreferrer noopener">{inlineMarkdown(label, depth + 1)}</a> : label;
    } else if (match[6] || match[7]) node = <strong>{inlineMarkdown(match[6] ?? match[7], depth + 1)}</strong>;
    else if (match[8]) node = <del>{inlineMarkdown(match[8], depth + 1)}</del>;
    else node = <em>{inlineMarkdown(match[9] ?? match[10], depth + 1)}</em>;
    nodes.push(<Fragment key={match.index}>{node}</Fragment>);
    offset = match.index! + match[0].length;
  }
  nodes.push(text.slice(offset));
  return nodes;
}

const listItem = /^(\s*)([-+*]|\d+[.)])\s+(.*)$/;
const heading = /^ {0,3}(#{1,6})\s+(.+?)(?:\s+#+)?\s*$/;
const rule = /^ {0,3}(?:(?:\*\s*){3,}|(?:-\s*){3,}|(?:_\s*){3,})$/;
const cells = (line: string) => line.trim().replace(/^\||\|$/g, '').split(/(?<!\\)\|/).map(cell => cell.trim().replace(/\\\|/g, '|'));
const tableRule = (line: string) => line.includes('|') && cells(line).every(cell => /^:?-{3,}:?$/.test(cell));
const startsBlock = (line: string) => heading.test(line) || rule.test(line) || listItem.test(line) || /^ {0,3}>/.test(line);

function blocks(text: string, depth = 0): ReactNode[] {
  if (depth > 12) return [<p key="text">{text}</p>];
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  const result: ReactNode[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const key = i;
    if (!line.trim()) { i++; continue; }
    const title = heading.exec(line);
    if (title) {
      const Tag = `h${title[1].length}` as 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6';
      result.push(<Tag key={key}>{inlineMarkdown(title[2])}</Tag>); i++; continue;
    }
    if (i + 1 < lines.length && /^ {0,3}(?:=+|-+)\s*$/.test(lines[i + 1])) {
      const Tag = lines[i + 1].trim().startsWith('=') ? 'h1' : 'h2';
      result.push(<Tag key={key}>{inlineMarkdown(line)}</Tag>); i += 2; continue;
    }
    if (rule.test(line)) { result.push(<hr key={key} />); i++; continue; }
    if (/^ {0,3}>/.test(line)) {
      const quote: string[] = [];
      while (i < lines.length && /^ {0,3}>/.test(lines[i])) quote.push(lines[i++].replace(/^ {0,3}> ?/, ''));
      result.push(<blockquote key={key}>{blocks(quote.join('\n'), depth + 1)}</blockquote>); continue;
    }
    if (i + 1 < lines.length && line.includes('|') && tableRule(lines[i + 1])) {
      const headers = cells(line);
      const alignments = cells(lines[i + 1]).map(cell => cell.startsWith(':') && cell.endsWith(':') ? 'center' : cell.endsWith(':') ? 'right' : 'left') as ('left' | 'right' | 'center')[];
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && lines[i].includes('|') && lines[i].trim()) rows.push(cells(lines[i++]));
      result.push(<div className="markdown-table" key={key}><table><thead><tr>{headers.map((cell, n) => <th key={n} style={{ textAlign: alignments[n] }}>{inlineMarkdown(cell)}</th>)}</tr></thead>
        <tbody>{rows.map((row, n) => <tr key={n}>{headers.map((_, c) => <td key={c} style={{ textAlign: alignments[c] }}>{inlineMarkdown(row[c] ?? '')}</td>)}</tr>)}</tbody></table></div>); continue;
    }
    const item = listItem.exec(line);
    if (item) {
      const ordered = /^\d/.test(item[2]);
      const indent = item[1].length;
      const items: ReactNode[] = [];
      while (i < lines.length) {
        const next = listItem.exec(lines[i]);
        if (!next || next[1].length !== indent || /^\d/.test(next[2]) !== ordered) break;
        const body = [next[3]];
        const start = i++;
        while (i < lines.length && lines[i].trim() && /^\s+/.test(lines[i]) && (lines[i].match(/^\s*/)?.[0].length ?? 0) > indent) {
          const continuation = lines[i++];
          body.push(continuation.slice(Math.min(indent + next[2].length + 1, continuation.match(/^\s*/)?.[0].length ?? 0)));
        }
        const task = /^\[([ xX])\]\s+/.exec(body[0]);
        if (task) body[0] = body[0].slice(task[0].length);
        items.push(<li key={start}>{task && <input type="checkbox" checked={task[1] !== ' '} disabled aria-label={task[1] === ' ' ? '未完成' : '已完成'} />}{blocks(body.join('\n'), depth + 1)}</li>);
      }
      result.push(ordered ? <ol key={key} start={parseInt(item[2], 10)}>{items}</ol> : <ul key={key}>{items}</ul>); continue;
    }
    const paragraph = [line]; i++;
    while (i < lines.length && lines[i].trim() && !startsBlock(lines[i]) && !(i + 1 < lines.length && (tableRule(lines[i + 1]) || /^ {0,3}(?:=+|-+)\s*$/.test(lines[i + 1])))) paragraph.push(lines[i++]);
    result.push(<p key={key}>{paragraph.map((part, n) => <Fragment key={n}>{n > 0 && <br />}{inlineMarkdown(part.replace(/ {2}$/, ''))}</Fragment>)}</p>);
  }
  return result;
}

export default function Markdown({ text }: { text: string }) {
  return <div className="markdown-content">{blocks(text)}</div>;
}