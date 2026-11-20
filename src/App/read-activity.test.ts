import { describe, expect, it } from 'vitest';
import { isReadCommand, markdownFileLabel, readToolLabel } from './read-activity';

describe('read operation attribution', () => {
  it('labels actual Markdown file operations by direction and basename', () => {
    expect(markdownFileLabel({ name: 'Read', input: { file_path: 'D:/资料/设计.md' } })).toBe('读入 · 设计.md');
    expect(markdownFileLabel({ name: 'Write', input: { file_path: 'D:\\资料\\结果.md' } })).toBe('输出 · 结果.md');
    expect(markdownFileLabel({ type: 'fileChange', changes: [{ path: 'docs/报告.md' }] })).toBe('输出 · 报告.md');
    expect(markdownFileLabel({ command: 'cat "docs/my notes.md"' })).toBe('读入 · my notes.md');
    expect(markdownFileLabel({ name: 'Write', input: { file_path: 'file.ts' } })).toBeUndefined();
  });
  it('recognizes file and web reads independently of document format', () => {
    for (const command of ['Get-Content "D:/资料.txt" | Select-Object -First 40', 'cat README.md', 'head -n 20 source.rs', 'curl https://example.com', 'powershell.exe -NoProfile -Command "Get-Content notes.txt"', '/bin/bash -lc \'cat notes.txt\'']) {
      expect(isReadCommand(command), command).toBe(true);
    }
    expect(readToolLabel({ name: 'mcp__files__read_file', input: { path: 'notes.txt' } })).toBe('读取资料 · mcp__files__read_file · notes.txt');
    expect(readToolLabel({ name: 'WebFetch', input: { url: 'https://example.com' } })).toBe('读取资料 · WebFetch · https://example.com');
  });

  it('does not classify answers, writes, tests or mixed command output as reads', () => {
    for (const command of ['npm test', 'echo "cat README.md"', 'npm test | grep failed', 'cat notes.txt; npm test', 'Set-Content notes.md value', 'git diff README.md']) {
      expect(isReadCommand(command), command).toBe(false);
    }
    expect(readToolLabel({ name: 'Write', input: { file_path: 'README.md' } })).toBeUndefined();
  });
});