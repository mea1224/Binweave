import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { formContent, type FormSchema } from './mcp-form';
import McpRequest from './McpRequest';
import type { Interaction } from '../types';

describe('MCP elicitation', () => {
  const schema: FormSchema = { properties: { count: { type: 'integer', minimum: 1, maximum: 3 }, agree: { type: 'boolean' }, text: { type: 'string', minLength: 1 }, many: { type: 'array', minItems: 1, items: { enum: ['a', 'b'] } } }, required: ['count', 'agree'] };
  it('preserves typed values, omission and enum arrays', () => {
    expect(formContent(schema, { count: '2', agree: false, many: ['b'] })).toEqual({ count: 2, agree: false, many: ['b'] });
    for (const values of [{ count: '', agree: false }, { count: '2.5', agree: false }, { count: '4', agree: false }, { count: '2' }, { count: '2', agree: 'false' }, { count: '2', agree: true, many: ['x'] }, { count: '2', agree: true, text: '' }]) expect(() => formContent(schema, values)).toThrow();
    expect(formContent({ properties: JSON.parse('{"__proto__":{"type":"string"}}') }, JSON.parse('{"__proto__":"safe"}'))).toHaveProperty('__proto__', 'safe');
  });
  const base: Interaction = { id: 'q', title: 'MCP 请求', detail: '原始消息', questions: [], approval: false, kind: 'mcp_url', mcpMode: 'url', mcpUrl: 'https://example.test/auth', status: 'pending', policy: 'ask' };
  const render = (r: Interaction) => renderToStaticMarkup(<McpRequest request={r} promptId="p" enabled respond={async () => true} />);
  it('shows explicit choices without opening links or claiming authentication', () => {
    const html = render(base);
    for (const text of ['接受', '拒绝', '取消', '复制链接', '不代表认证已成功', 'https://example.test/auth']) expect(html).toContain(text);
    expect(html).not.toContain('href=');
    expect(render({ ...base, status: 'expired' })).not.toContain('<form');
    expect(render({ ...base, kind: 'mcp_unsupported' })).toContain('<button disabled="">接受');
  });
  it('renders schema fields and hides answered interactions after reload', () => {
    const html = render({ ...base, kind: 'mcp_form', mcpSchema: schema as unknown as Record<string, unknown> });
    expect(html).toContain('type="number"'); expect(html).toContain('type="checkbox"'); expect(html).toContain('必填');
    for (const decision of ['accept', 'decline', 'cancel']) {
      for (const kind of ['mcp_form', 'mcp_url', 'mcp_confirmation', 'mcp_unsupported'] as const) {
        expect(render({ ...base, kind, status: 'answered', decision, delivery: 'delivered' })).toBe('');
      }
    }
  });
  it('keeps failed delivery and expired requests visible', () => {
    expect(render({ ...base, status: 'expired', decision: 'accept', delivery: 'failed' })).toContain('响应写入失败');
    expect(render({ ...base, status: 'expired' })).toContain('请求已过期');
    expect(render({ ...base, status: 'expired', decision: 'accept', delivery: 'delivered' })).toBe('');
    expect(render({ ...base, status: 'pending', decision: 'decline' })).toBe('');
    expect(render({ ...base, status: 'expired', decision: 'accept', delivery: 'failed' })).not.toContain('MCP 用户交互');
  });
});