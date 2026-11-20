import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import App from './index';

describe('控制台布局', () => {
  it('保留时间轴，移除笔记和审计入口并提供删除记录', () => {
    const html = renderToStaticMarkup(<App />);
    expect(html).not.toContain('从证据到实现');
    expect(html).not.toContain('每次交接，都有据可查');
    expect(html).not.toContain('class="hero"');
    expect(html).not.toContain('最近活动');
    expect(html).not.toContain('开始分析 · 尚未开放');
    expect(html).toContain('协作时间轴');
    expect(html).not.toContain('<h3>操作笔记</h3>');
    expect(html).not.toContain('id="note"');
    expect(html).not.toContain('保存笔记');
    expect(html).not.toContain('审计记录');
    expect(html).toContain('删除记录…');
    expect(html).toContain('保存工作区');
    expect(html).not.toContain('本地工作区');
    expect(html).not.toContain('workspace-name');
    expect(html).toContain('工作区列表');
    expect(html).toContain('class="mcp-policy-toolbar"');
    expect(html).toContain('class="mcp-policy-control"');
    expect(html).toContain('aria-describedby="mcp-policy-help"');
    expect(html).toContain('<summary>策略说明</summary>');
    expect(html).not.toContain('<section class="panel"><label for="mcp-policy">');
  });
});