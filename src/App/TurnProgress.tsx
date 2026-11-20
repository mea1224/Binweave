import { useEffect, useMemo, useRef, useState } from 'react';
import type { AgentEvent, Interaction, RespondInteraction, Turn } from '../types';
import { formatTime } from '../view-model';
import type { Conversation } from './conversation';
import { displayEvents } from './display-events';
import { formatDuration } from './duration';
import McpRequest from './McpRequest';

function RequestForm({ request, promptId, enabled, respond }: { request: Interaction; promptId: string; enabled: boolean; respond?: RespondInteraction }) {
  const [answers, setAnswers] = useState<Record<string, string[]>>({});
  const [custom, setCustom] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState(false);
  const [error, setError] = useState('');
  const locked = useRef(false);
  const pending = request.status === 'pending' && !sent;
  const disabled = !enabled || !respond || busy || !pending;
  const submit = async (decision?: 'allow' | 'deny', remember = false) => {
    if (disabled || locked.current) return;
    locked.current = true; setBusy(true); setError('');
    const values = Object.fromEntries(request.questions.map(q => [q.id, custom[q.id]?.trim() ? [...(q.multiple ? answers[q.id] ?? [] : []), custom[q.id]] : answers[q.id] ?? []]));
    try {
      if (await respond!(promptId, request.id, request.approval ? { decision, remember } : { answers: values })) setSent(true);
      else setError('回答未确认送达，请检查任务状态后重试。');
    } catch (e) { setError(String(e)); }
    finally { locked.current = false; setBusy(false); }
  };
  // All completed choices disappear, including CLI questions wrapping MCP calls.
  if (sent || request.status === 'answered') return null;
  return <section className="agent-interaction" aria-label="任务内交互">
    <strong>{request.title}</strong>
    {request.status === 'blocked' && <p role="alert">此侧 CLI 请求被权限策略阻止。不要反复尝试绕过；可先停止当前执行并人工补充证据，或调整合规读取方式后使用“仅重启此侧并重试”。另一侧会话和历史不会清除。</p>}
    <p role={pending ? 'status' : undefined}>{pending ? enabled ? request.approval ? '需要您的授权：请选择允许本次请求或拒绝，选择将回传当前 Agent。等待选择期间暂停 5 分钟思考计时。' : '等待您的回答，回答将返回当前 Agent。' : '此请求当前不可回复。' : request.status === 'blocked' ? '策略拒绝已记录，未授予权限。' : request.status === 'expired' ? '请求已过期，未自动重放。' : '响应已写入 Agent 通道。'}</p>
    <details open={pending && request.approval}><summary>请求详情</summary><pre>{request.detail}</pre></details>
    {pending && request.approval && request.canRemember && <p>记住后，仅当前工作区的当前 Agent 在相同执行目录、相同命令和请求参数下不再询问；重启应用后失效。不适用于其他命令或新增权限。</p>}
    {pending && <form onSubmit={event => { event.preventDefault(); void submit(); }}>
      {request.questions.map((q, index) => <fieldset key={q.id} disabled={disabled}>
        <legend>{q.text}{q.multiple ? '（可多选）' : ''}</legend>
        {q.options.map(option => <label className="interaction-option" key={option}><input type={q.multiple ? 'checkbox' : 'radio'} name={`${request.id}-${index}`} checked={(answers[q.id] ?? []).includes(option)} onChange={event => {
          setAnswers(current => ({ ...current, [q.id]: q.multiple ? event.target.checked ? [...(current[q.id] ?? []), option] : (current[q.id] ?? []).filter(v => v !== option) : [option] }));
          if (!q.multiple) setCustom(current => ({ ...current, [q.id]: '' }));
        }} />{option}</label>)}
        <label htmlFor={`${request.id}-${index}-text`}>手动输入回答</label>
        <textarea id={`${request.id}-${index}-text`} rows={2} maxLength={16000} value={custom[q.id] ?? ''} onChange={event => { setCustom(current => ({ ...current, [q.id]: event.target.value })); if (!q.multiple) setAnswers(current => ({ ...current, [q.id]: [] })); }} />
      </fieldset>)}
      <div className="actions">{request.approval ? <><button type="button" disabled={disabled} onClick={() => void submit('allow')}>允许本次请求</button>{request.canRemember && <button type="button" disabled={disabled} onClick={() => void submit('allow', true)}>允许，此命令不再询问</button>}<button type="button" disabled={disabled} onClick={() => void submit('deny')}>拒绝</button></> : <button disabled={disabled || request.questions.some(q => !custom[q.id]?.trim() && !answers[q.id]?.length)}>提交回答</button>}{busy && <span role="status">正在发送…</span>}</div>
    </form>}
    {error && <p role="alert">{error}</p>}
  </section>;
}

export default function TurnProgress({ turn, conversation, open, active, enabled, respond, visibleEvents }: { turn?: Turn; conversation: Conversation; open: boolean; active: boolean; enabled: boolean; respond?: RespondInteraction; visibleEvents?: AgentEvent[] }) {
  const [now, setNow] = useState(Date.now());
  const requests = (turn?.interactions ?? []).filter(request => request.delivery === 'failed' || request.status !== 'answered' && !request.decision);
  const waiting = active && requests.some(r => r.status === 'pending');
  const running = active && ['submitted', 'running'].includes(turn?.status ?? '');
  useEffect(() => { if (!running) return; const timer = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(timer); }, [running]);
  const last = conversation.events.at(-1);
  const activity = useMemo(() => [...(visibleEvents ?? displayEvents(conversation.events))].reverse().find(e => e.kind === 'tool_summary' || e.kind === 'message'), [visibleEvents, conversation.events]);
  const seconds = last ? Math.max(0, Math.floor((now - Date.parse(last.createdAt)) / 1000)) : 0;
  const startedAt = conversation.events.find(event => event.id === conversation.promptId)?.createdAt ?? conversation.createdAt;
  const elapsed = ((running ? Math.max(now, Date.parse(last?.createdAt ?? startedAt)) : Date.parse(last?.createdAt ?? startedAt)) - Date.parse(startedAt)) / 1000;
  const failed = ['failed', 'interrupted'].includes(turn?.status ?? '');
  const thinking = running && !waiting && (turn?.status === 'submitted' || !activity || activity.kind === 'tool_summary' || seconds >= 3);
  const tone = failed ? 'error' : thinking || waiting || turn?.status === 'cancelled' ? 'waiting' : turn ? 'healthy' : 'neutral';
  const status = waiting ? requests.some(r => r.status === 'pending' && r.approval) ? '等待用户授权' : '等待用户回答' : thinking ? '正在思考，等待输出…' : turn ? { submitted: '正在启动', running: '正在输出', completed: '已完成', failed: '异常退出', interrupted: '执行中断', cancelled: '用户已停止' }[turn.status] : '历史记录';
  return <div className={`turn-progress ${tone}`}>
    <div role="status"><strong>{(thinking || waiting) && !failed ? <span className="thinking-spinner" aria-hidden="true" /> : '● '}工作状态：{status}</strong>{running && Number.isFinite(seconds) && <span> · 距最近活动 {formatDuration(seconds)}</span>}{Number.isFinite(elapsed) && <span> · {running ? '任务已用时' : '任务用时'} {formatDuration(elapsed)}</span>}</div>
    <p>显示进度：已接收 {conversation.events.length} 条记录{last ? ` · 更新至 #${last.sequence}（${formatTime(last.createdAt)}）` : ''} · {open ? '内容已展开' : '内容已收起'}</p>
    {failed && activity && <p>最近工作进展：{activity.content.slice(0, 240)}{activity.content.length > 240 ? '…' : ''}</p>}
    {turn?.detail && (!running || turn.status === 'submitted') && <pre className="turn-detail">{turn.detail}</pre>}
    {running && !waiting && seconds >= 30 && <p>暂未收到新输出，正在等待 Agent；连续思考或等待有效输出达到 5 分钟将自动终止并报告超时。</p>}
    {requests.map(request => request.kind?.startsWith('mcp_') ? <McpRequest key={request.id} request={request} promptId={turn!.promptId} enabled={enabled && running} respond={respond} /> : <RequestForm key={request.id} request={request} promptId={turn!.promptId} enabled={enabled && running} respond={respond} />)}
  </div>;
}