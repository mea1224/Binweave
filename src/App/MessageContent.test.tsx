import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import MessageContent, { codeLanguage, splitContent, isLongMarkdown } from './MessageContent';
import Markdown from './Markdown';

describe('代码块', () => {
  it('shows an explicitly supplied output filename instead of a Markdown label', () => {
    const html = renderToStaticMarkup(<MessageContent text={'```markdown filename=docs/report.md\n# 报告\n' + '正文'.repeat(650) + '\n```'} />);
    expect(html).toContain('输出 · report.md');
    expect(html).not.toContain('Markdown');
  });
  it('短 Markdown 会话始终直接解析展示，不折叠', () => {
    for (const text of ['# 分析报告\n内容', '**结论**：内容', '- 第一项\n- 第二项', '[链接](https://example.com)', '| 表头 |\n| --- |']) {
      const loading = renderToStaticMarkup(<MessageContent text={text} settled={false} />);
      expect(loading).toContain('markdown-content');
      expect(loading).not.toContain('双击预览');
      expect(loading).not.toContain('<pre>');
      expect(loading).not.toContain('展开 Markdown');
      const ready = renderToStaticMarkup(<MessageContent text={text} />);
      expect(ready).not.toContain('展开 Markdown');
      expect(ready).not.toContain('<pre>');
    }
  });
  it('短 Markdown 围栏也解析展示，正文内嵌代码独立折叠', () => {
    expect(renderToStaticMarkup(<MessageContent text={'```markdown\n# 标题\n```'} />)).toContain('<h1>标题</h1>');
    const html = renderToStaticMarkup(<MessageContent text={'# 标题\n```js\nconst x = 1;\n```'} />);
    expect(html).toContain('<h1>标题</h1>');
    expect(html).not.toContain('const x');
    expect(html).toContain('展开代码');
  });
  it('完整围栏默认折叠，保留代码和语言', () => {
    const text = '说明\n```ts\nconst value: number = 1;\n```\n结束';
    const parts = splitContent(text, false);
    expect(parts[1]).toEqual({ code: true, text: 'const value: number = 1;\n', language: 'TypeScript', complete: true });
    const html = renderToStaticMarkup(<MessageContent text={text} />);
    expect(html).toContain('TypeScript');
    expect(html).toContain('展开代码');
    expect(html).toContain('代码加载完成');
    expect(html).not.toContain('const value');
    expect(html).toContain('结束');
  });
  it('流式未闭合代码提供双击预览，结束后提示加载完毕', () => {
    const text = '```python\ndef test():\n    return 1';
    expect(renderToStaticMarkup(<MessageContent text={text} settled={false} />)).toContain('代码加载中');
    expect(renderToStaticMarkup(<MessageContent text={text} settled={false} />)).toContain('双击预览');
    expect(renderToStaticMarkup(<MessageContent text={text} />)).toContain('代码加载完成');
    expect(splitContent(text, false)[0].complete).toBe(false);
    expect(splitContent(text, true)[0].complete).toBe(true);
  });
  it('保守检测语言，支持波浪围栏，不执行 HTML', () => {
    expect(codeLanguage('', 'fn main() {}')).toBe('Rust');
    expect(codeLanguage('', '{"a": 1}')).toBe('JSON');
    expect(codeLanguage('', '不是代码')).toBe('未识别语言');
    expect(splitContent('~~~js\nalert(1)\n~~~', true)[0].language).toBe('JavaScript');
    expect(renderToStaticMarkup(<MessageContent text={'<script>alert(1)</script>'} />)).not.toContain('<script>');
  });
  it('只折叠超过 1200 字符或 24 行的长内容，完成后标题栏可操作', () => {
    expect(isLongMarkdown('a'.repeat(1200))).toBe(false);
    expect(isLongMarkdown('a'.repeat(1201))).toBe(true);
    expect(isLongMarkdown(Array(24).fill('短行').join('\n'))).toBe(false);
    const text = Array(25).fill('**长文内容**').join('\n');
    expect(isLongMarkdown(text)).toBe(true);
    const loading = renderToStaticMarkup(<MessageContent text={text} settled={false} />);
    expect(loading).not.toContain('加载中');
    expect(loading).not.toContain('role="status"');
    expect(loading).toContain('双击预览');
    expect(loading).not.toContain('长文内容');
    const ready = renderToStaticMarkup(<MessageContent text={text} />);
    expect(ready).toContain('内容加载完成');
    expect(ready).toContain('回复内容');
    expect(ready).not.toContain('Markdown');
    expect(ready).toContain('role="button"');
    expect(ready).not.toContain('双击预览');
    expect(ready).not.toContain('<button');
  });
  it('解析标题、强调、引用、任务列表、嵌套列表、表格及行内代码', () => {
    const html = renderToStaticMarkup(<Markdown text={'# 标题\n\n**粗体** *斜体* ~~删除~~ `const x`\n\n> 引用\n\n- [x] 完成\n  - 子项\n\n3. 第三项\n\n| 名称 | 值 |\n| --- | ---: |\n| **结果** | `1` |'} />);
    for (const tag of ['<h1>标题</h1>', '<strong>粗体</strong>', '<em>斜体</em>', '<del>删除</del>', 'class="inline-code"', '<blockquote>', 'checked=""', '<ul>', '<ol start="3">', '<table>', '<strong>结果</strong>']) expect(html).toContain(tag);
    expect(html.match(/<ul>/g)).toHaveLength(2);
  });
  it('转义 HTML、拒绝危险链接、保留代码原文且不加载外部图片', () => {
    const html = renderToStaticMarkup(<Markdown text={'<script>alert(1)</script>\n[危险](javascript:alert) [安全](https://example.com)\n![图片](https://example.com/image.png)\n`**原文**` user_name_value'} />);
    expect(html).not.toContain('<script>');
    expect(html).not.toContain('href="javascript:');
    expect(html).not.toContain('<img');
    expect(html).toContain('href="https://example.com"');
    expect(html).toContain('>**原文**</code>');
    expect(html).toContain('user_name_value');
  });
});