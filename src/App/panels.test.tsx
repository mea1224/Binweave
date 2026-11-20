import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { Activity, Diagnostics, CheckpointView, RoleCard, SkillHistory } from './panels';
import App from './index';

describe('Simplified Chinese workspace panels', () => {
  it('shows honest empty states instead of simulated worker activity', () => {
    const html = renderToStaticMarkup(<Activity events={[]} empty="暂无工作进程输出。" />);
    expect(html).toContain('暂无工作进程输出。');
    expect(html).not.toContain('<li');
  });
  it('escapes agent content and preserves its original language', () => {
    const html = renderToStaticMarkup(<Activity empty="无" events={[{ id: 'a', runId: 'r', sequence: 1, createdAt: '2026-11-19T10:00:00Z', agent: 'codex', kind: 'stdout', content: '<script>原文</script>', parentId: null }]} />);
    expect(html).toContain('原文');
    expect(html).toContain('&lt;script&gt;');
    expect(html).not.toContain('<script>');
  });
  it('does not present diagnostics or recovery as verified before loading', () => {
    expect(renderToStaticMarkup(<Diagnostics report={null} checking={false} />)).toContain('暂无诊断报告');
    expect(renderToStaticMarkup(<CheckpointView snapshot={null} directory="" disabled onAcknowledge={() => {}} />)).toContain('尚未加载检查点');
    expect(renderToStaticMarkup(<SkillHistory artifacts={[]} />)).toContain('暂无已保存的技能契约');
  });
  it('localizes the shell and keeps unavailable actions disabled', () => {
    const html = renderToStaticMarkup(<App />);
    expect(html).toContain('工作区导航');
    expect(html).toContain('浏览器预览仅供查看');
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>发送给 Codex<\/button>/);
    expect(html).not.toContain('Control room');
    expect(html).not.toContain('Foundation release');
  });
  it('keeps role behavior independent of translated labels', () => {
    const html = renderToStaticMarkup(<RoleCard role="analyst" agent="claude" ready />);
    expect(html).toContain('分析员');
    expect(html).toContain('Claude Code');
    expect(html).toContain('REA MCP 已验证');
    expect(renderToStaticMarkup(<RoleCard role="implementer" agent="codex" ready={false} />)).toContain('实现契约');
  });
  it('localizes diagnostic details while preserving paths and tool names', () => {
    const html = renderToStaticMarkup(<Diagnostics checking={false} report={{
      checkedAt: '2026-11-19T10:00:00Z', platform: 'windows',
      checks: [{ id: 'config', label: 'Codex MCP configuration', status: 'ok', detail: 'Configuration parsed. Credentials are never included in the report.', path: 'D:/workspace/config.toml' }],
      mcpServers: [{ id: 'rea', name: 'rea (Codex, global)', owner: 'codex', transport: 'stdio', source: 'D:/workspace/config.toml', status: 'ok', detail: '实际调用验证可用 1 个工具', tools: ['read_status'], toolChecks: [{ name: 'read_status', status: 'ok', detail: '只读调用成功' }, { name: 'open_binary', status: 'not_checked', detail: '需要业务参数' }] }],
    }} />);
    expect(html).toContain('Codex MCP 配置');
    expect(html).toContain('已验证可用 1 个工具');
    expect(html).toContain('read_status');
    expect(html).toContain('未验证可用的工具及原因');
    expect(html).toContain('需要业务参数');
    expect(html).not.toContain('已声明');
    expect(html).toContain('rea （Codex，全局）');
    expect(html).toContain('D:/workspace/config.toml');
    expect(html).toContain('open_binary');
    expect(html).not.toContain('Configuration parsed');
  });
  it('不把旧报告中的声明目录当成已验证工具', () => {
    const html = renderToStaticMarkup(<Diagnostics checking={false} report={{ checkedAt: '', platform: 'windows', checks: [], mcpServers: [{ id: 'old', name: 'rea', owner: 'codex', transport: 'stdio', source: '', status: 'ok', detail: '', tools: ['unverified_tool'] }] }} />);
    expect(html).toContain('已验证可用 0 个工具');
    expect(html).toContain('请重新运行诊断');
    expect(html).not.toContain('unverified_tool');
  });
  it('区分连接、上下文待测、审批与单工具失败并保留成功工具', () => {
    const html = renderToStaticMarkup(<Diagnostics checking={false} report={{ checkedAt: '', platform: 'windows', checks: [], mcpServers: [{ id: 'rea', name: 'rea', owner: 'codex', transport: 'stdio', source: '', status: 'ok', connectionStatus: 'ok', detail: '', tools: ['procedure_info'], toolChecks: [
      { name: 'procedure_info', status: 'ok', outcome: 'verified', method: 'context', detail: '查询成功' },
      { name: 'search_strings', status: 'not_checked', outcome: 'needs_context', method: 'none', detail: '' },
      { name: 'set_comment', status: 'not_checked', outcome: 'approval_required', method: 'none', detail: '' },
      { name: 'read_bytes', status: 'error', outcome: 'unavailable', method: 'context', detail: '' },
      { name: 'xrefs', status: 'error', outcome: 'failed', method: 'context', detail: '' },
      { name: 'list_names', status: 'not_checked', outcome: 'interrupted', method: 'none', detail: '' },
    ] }] }} />);
    for (const text of ['连接握手', '已验证可用 1 个工具', '上下文参数验证', '需目标／业务参数待测', '需审批／写操作未执行', '环境缺失／明确不支持', '调用失败', '验证中断']) expect(html).toContain(text);
  });
});