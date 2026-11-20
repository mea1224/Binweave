import { memo, useId, useMemo, useState, type ReactNode } from 'react';
import Markdown from './Markdown';

export interface ContentPart { text: string; code: boolean; language?: string; complete?: boolean; filename?: string }
export function codeLanguage(hint: string, text: string): string {
  if (hint.toLowerCase() === 'markdown') return 'Markdown';
  const aliases: Record<string, string> = { js: 'JavaScript', javascript: 'JavaScript', ts: 'TypeScript', typescript: 'TypeScript', tsx: 'TSX', jsx: 'JSX', py: 'Python', python: 'Python', rs: 'Rust', rust: 'Rust', cpp: 'C++', 'c++': 'C++', c: 'C', cs: 'C#', csharp: 'C#', java: 'Java', go: 'Go', json: 'JSON', html: 'HTML', xml: 'XML', css: 'CSS', sql: 'SQL', sh: 'Shell', bash: 'Shell', shell: 'Shell', powershell: 'PowerShell', ps1: 'PowerShell', yaml: 'YAML', yml: 'YAML', toml: 'TOML', md: 'Markdown', text: '纯文本', plaintext: '纯文本' };
  if (hint) return aliases[hint.toLowerCase()] ?? hint;
  try { JSON.parse(text); return 'JSON'; } catch { /* Continue with conservative signatures. */ }
  if (/^\s*(?:from \w+ import |def \w+\(|import (?:os|sys|re)\b)/m.test(text)) return 'Python';
  if (/\b(?:fn \w+\(|let mut |impl \w+|use std::)/.test(text)) return 'Rust';
  if (/^\s*#include\s*[<"]|\bstd::/.test(text)) return text.includes('std::') ? 'C++' : 'C';
  if (/\b(?:interface \w+|type \w+\s*=|const \w+:\s*\w+)/.test(text)) return 'TypeScript';
  if (/\b(?:const |let |function |console\.log|=>)/.test(text)) return 'JavaScript';
  if (/^\s*(?:SELECT .+ FROM|CREATE TABLE|INSERT INTO)/im.test(text)) return 'SQL';
  if (/^\s*<(!DOCTYPE|html|div|\?xml)\b/i.test(text)) return 'HTML';
  if (/^#!.*(?:bash|sh)\b/m.test(text)) return 'Shell';
  return '未识别语言';
}

export function splitContent(text: string, settled: boolean): ContentPart[] {
  const parts: ContentPart[] = [];
  const fence = /^[ \t]*(`{3,}|~{3,})([^\r\n]*)\r?\n/gm;
  let offset = 0;
  let match: RegExpExecArray | null;
  while ((match = fence.exec(text))) {
    if (match.index > offset) parts.push({ text: text.slice(offset, match.index), code: false });
    const end = new RegExp(`^[ \\t]*${match[1][0]}{${match[1].length},}[ \\t]*\\r?$`, 'gm');
    end.lastIndex = fence.lastIndex;
    const close = end.exec(text);
    const code = text.slice(fence.lastIndex, close?.index ?? text.length);
    parts.push({ text: code, code: true, language: codeLanguage(match[2].trim().split(/\s/)[0], code), complete: !!close || settled });
    const filename = match[2].match(/(?:^|\s)(?:filename=)?["']?([^\s"']+\.(?:md|markdown))["']?(?:\s|$)/i)?.[1];
    if (filename) parts[parts.length - 1].filename = filename.split(/[\\/]/).at(-1);
    offset = close ? end.lastIndex : text.length;
    fence.lastIndex = offset;
  }
  if (offset < text.length) parts.push({ text: text.slice(offset), code: false });
  return parts;
}

function Highlight({ text, language }: { text: string; language: string }) {
  const tokens = useMemo(() => {
    const hashComment = ['Python', 'Shell', 'PowerShell', 'YAML', 'TOML'].includes(language);
    const pattern = /("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`|\/\/[^\n]*|\/\*[\s\S]*?\*\/|#[^\n]*|\b\d+(?:\.\d+)?\b|\b(?:const|let|var|function|return|if|else|for|while|class|interface|type|import|from|export|default|async|await|def|fn|pub|use|impl|struct|enum|match|mut|true|false|null|None|True|False|new|throw|try|catch|SELECT|FROM|WHERE|INSERT|INTO|CREATE|TABLE)\b)/g;
    return text.split(pattern).map((part, i) => {
      let kind = '';
      if (/^["'`]/.test(part)) kind = 'string';
      else if (/^\/[/\*]/.test(part) || (hashComment && part.startsWith('#'))) kind = 'comment';
      else if (/^\d/.test(part)) kind = 'number';
      else if (i % 2 === 1 && !part.startsWith('#')) kind = 'keyword';
      return kind ? <span key={i} className={`syntax-${kind}`}>{part}</span> : part;
    });
  }, [text, language]);
  return <code>{tokens}</code>;
}

function ContentBlock({ title, label, complete, children, className = '', lines, collapsible = true, showLoading = true }: {
  title: string; label: string; complete: boolean; children: ReactNode; className?: string; lines?: number; collapsible?: boolean; showLoading?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const contentId = useId();
  const headingId = useId();
  const collapse = `收起${label}`;
  if (!collapsible) return <>{children}</>;
  return <div className={`code-block ${className}`}>
    <div className="code-heading" id={headingId} role="button" tabIndex={0} aria-controls={contentId} aria-expanded={open}
      onClick={event => { if (complete && event.detail < 2) setOpen(value => !value); }}
      onDoubleClick={() => { if (!complete) setOpen(true); }}
      onKeyDown={event => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); setOpen(value => !value); } }}>
      <strong>{title}</strong>{lines !== undefined && <span>{lines} 行</span>}
      {(complete || showLoading) && <span role="status">{label.trim()}{complete ? '加载完成' : '加载中…'}</span>}
      <span className="code-toggle">{open ? collapse : complete ? `展开${label}` : '双击预览（键盘按回车）'}</span>
    </div>
    {open && <div id={contentId}>
      {children}
      <div className="code-footer"><button type="button" onClick={() => {
        setOpen(false);
        document.getElementById(headingId)?.focus({ preventScroll: true });
      }}>{collapse}</button></div>
    </div>}
  </div>;
}

function CodeBlock({ part }: { part: ContentPart }) {
  if (part.language === 'Markdown') return <MarkdownBlock text={part.text} settled={!!part.complete} filename={part.filename} />;
  return <ContentBlock title={part.language ?? '未识别语言'} label={part.language === 'Markdown' ? ' Markdown' : '代码'} complete={!!part.complete} lines={part.text.split('\n').length}>
    <pre><Highlight text={part.text} language={part.language ?? ''} /></pre>
  </ContentBlock>;
}

export function isMarkdown(text: string): boolean {
  return /^(?: {0,3}(?:#{1,6}\s|>\s|[-+*]\s|\d+[.)]\s)|\s*\|.+\||\s*(?:-{3,}|\*{3,}|_{3,}|={3,})\s*$)/m.test(text)
    || /(?:\*\*[^\n]+\*\*|__[^\n]+__|\*[^*\n]+\*|_[^_\n]+_|~~[^\n]+~~|!?\[[^\]\n]+\]\([^\n)]+\)|`[^`\n]+`)/.test(text);
}

export function isLongMarkdown(text: string): boolean {
  return text.length > 1200 || text.trim().split(/\r?\n/).length > 24;
}

function ContentParts({ parts }: { parts: ContentPart[] }) {
  return <>{parts.map((part, i) => part.code ? <CodeBlock key={i} part={part} /> : <Markdown key={i} text={part.text} />)}</>;
}

function MarkdownBlock({ text, settled, children, filename }: { text: string; settled: boolean; children?: ReactNode; filename?: string }) {
  return <ContentBlock title={filename ? `输出 · ${filename}` : '回复内容'} label="内容" showLoading={false} complete={settled} className="markdown-block" collapsible={isLongMarkdown(text)}>
    {children ?? <ContentParts parts={splitContent(text, settled)} />}
  </ContentBlock>;
}

export default memo(function MessageContent({ text, settled = true }: { text: string; settled?: boolean }) {
  const parts = useMemo(() => splitContent(text, settled), [text, settled]);
  const markdown = parts.some(part => !part.code && isMarkdown(part.text));
  if (!markdown) return <ContentParts parts={parts} />;
  const files = parts.filter(part => part.filename).map(part => part.filename);
  return <MarkdownBlock text={text} settled={settled} filename={files.length === 1 ? files[0] : undefined}><ContentParts parts={parts} /></MarkdownBlock>;
});