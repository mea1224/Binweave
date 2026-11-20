import { useRef, useState } from 'react';
import type { Interaction, McpElicitationAction, RespondInteraction } from '../types';
import { formContent, options, type FormSchema } from './mcp-form';

export default function McpRequest({ request, promptId, enabled, respond }: { request: Interaction; promptId: string; enabled: boolean; respond?: RespondInteraction }) {
  const schema = (request.mcpSchema ?? { properties: {} }) as unknown as FormSchema;
  const [values, setValues] = useState<Record<string, unknown>>(() => Object.fromEntries(Object.entries(schema.properties).filter(([, f]) => f.default != null).map(([key, f]) => [key, f.default])));
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState<McpElicitationAction>();
  const [error, setError] = useState('');
  const lock = useRef(false);
  const pending = request.status === 'pending' && !sent;
  const disabled = !enabled || !respond || !pending || busy;
  const set = (name: string, value: unknown) => setValues(previous => ({ ...previous, [name]: value }));
  async function submit(action: McpElicitationAction) {
    if (disabled || lock.current) return;
    lock.current = true; setBusy(true); setError('');
    try {
      const content = action === 'accept' && (request.kind === 'mcp_form' || request.kind === 'mcp_confirmation') ? formContent(schema, values) : undefined;
      if (await respond!(promptId, request.id, { action, ...(content === undefined ? {} : { content }) })) { setSent(action); setValues({}); }
      else setError('回答未确认送达，请检查任务状态后重试。');
    } catch (e) { setError(String(e)); }
    finally { lock.current = false; setBusy(false); }
  }
  // A later expiry must not bring a completed choice back into the card.
  if (sent || request.decision || request.status === 'answered') {
    return request.delivery === 'failed' ? <p role="alert">交互响应写入失败，请检查恢复检查点。</p> : null;
  }
  const decision = sent ?? request.decision;
  return <section className="agent-interaction" aria-label="MCP 用户交互">
    <strong>{request.title}</strong><pre>{request.detail}</pre>
    <p>策略来源：{request.policy === 'decline' ? '工作区无人值守拒绝' : request.policy === 'cancel' ? '工作区无人值守取消' : '询问用户'} · 模式：{request.mcpMode}</p>
    <p role="status">{pending ? '等待您的明确选择；执行计时与自动交接已暂停。' : request.status === 'expired' ? '请求已过期，不会自动重放。' : `选择：${decision === 'accept' ? '接受' : decision === 'decline' ? '拒绝' : decision === 'cancel' ? '取消' : '已回答'}；响应已写入原 Agent 通道。`}</p>
    {request.delivery === 'failed' && <p role="alert">响应写入失败，请检查恢复检查点；不要假定服务端已收到选择。</p>}
    {request.mcpUrl && <><pre>{request.mcpUrl}</pre><button type="button" onClick={() => void navigator.clipboard.writeText(request.mcpUrl!).catch(() => setError('复制失败，请手动选择并复制链接。'))}>复制链接</button><p>请自行核实链接并在浏览器处理。本应用不会自动打开链接；选择接受不代表认证已成功。</p></>}
    {pending && <form onSubmit={event => { event.preventDefault(); void submit('accept'); }}>
      {Object.entries(schema.properties).map(([name, field], index) => {
        const choices = options(field); const value = values[name]; const id = `${request.id}-mcp-${index}`;
        return <fieldset key={name} disabled={disabled}><legend>{field.title ?? name}{schema.required?.includes(name) ? '（必填）' : '（可选）'}</legend>
          {field.description && <p>{field.description}</p>}
          {field.type === 'array' ? choices.map(choice => <label className="interaction-option" key={choice.value}><input type="checkbox" checked={Array.isArray(value) && value.includes(choice.value)} onChange={e => set(name, e.target.checked ? [...(Array.isArray(value) ? value : []), choice.value] : (Array.isArray(value) ? value : []).filter(v => v !== choice.value))} />{choice.label}</label>) : field.type === 'boolean' ? <select aria-label={field.title ?? name} value={value === undefined ? '' : String(value)} onChange={e => set(name, e.target.value === '' ? undefined : e.target.value === 'true')}><option value="">请选择</option><option value="true">是</option><option value="false">否</option></select> : choices.length ? <select aria-label={field.title ?? name} value={value === undefined ? '' : String(choices.findIndex(o => o.value === value))} onChange={e => set(name, e.target.value === '' ? undefined : choices[Number(e.target.value)].value)}><option value="">请选择</option>{choices.map((o, i) => <option key={o.value} value={i}>{o.label}</option>)}</select> : <><label htmlFor={id}>{name}</label><input id={id} type={field.type === 'number' || field.type === 'integer' ? 'number' : 'text'} step={field.type === 'integer' ? 1 : 'any'} value={value === undefined ? '' : String(value)} onChange={e => set(name, e.target.value)} /></>}
          {!schema.required?.includes(name) && <button type="button" onClick={() => set(name, undefined)}>不提供此字段</button>}
        </fieldset>;
      })}
      <div className="actions"><button disabled={disabled || request.kind === 'mcp_unsupported'}>接受</button><button type="button" disabled={disabled} onClick={() => void submit('decline')}>拒绝</button><button type="button" disabled={disabled} onClick={() => void submit('cancel')}>取消</button>{busy && <span role="status">正在发送…</span>}</div>
      <p>拒绝或取消后将停止自动交接，保留人工恢复检查点。表单答案不会作为新提示词发送，也不会保存到交互审计记录。</p>
    </form>}
    {error && <p role="alert">{error}</p>}
  </section>;
}