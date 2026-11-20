import { memo, useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import type { liveSnapshot } from '../live-snapshot';
import { useAutoFollow } from './useAutoFollow';
import MessageContent from './MessageContent';
import type { AgentEvent, AgentId, RunSnapshot, RespondInteraction, RoleStamp } from '../types';
import TurnProgress from './TurnProgress';
import { eventContent, formatTime, label, resolveEdges } from '../view-model';
import { conversations, conversationProjection, type Conversation } from './conversation';
import { displayProjection } from './display-events';

const names = { codex: 'Codex', claude: 'Claude' };
export function historicalRole(identity?: RoleStamp) {
  return identity?.logicalRole ? `${identity.logicalRole === 'analyst' ? '分析方' : '实现方'} · 角色版本 ${identity.roleRevision ?? '未知'}` : '历史角色未记录';
}
const EventRow = memo(function EventRow({ event, settled }: { event: AgentEvent; settled: boolean }) {
  return <section className="conversation-event">
    {event.kind === 'stdout' || event.kind === 'stderr' || event.kind === 'system' ? <details><summary>{label(event.kind)} · {formatTime(event.createdAt)}</summary><pre>{eventContent(event)}</pre></details> : <>
      <div className="event-meta"><strong>{label(event.kind)}</strong><time dateTime={event.createdAt}>{formatTime(event.createdAt)}</time></div>
      <MessageContent text={eventContent(event)} settled={settled} />
    </>}
  </section>;
});
export function deliveryStatus(snapshot: RunSnapshot | null, promptId: string) {
  const status = snapshot?.run.execution?.turns.find(turn => turn.promptId === promptId)?.status;
  return status ? { submitted: '已提交', running: '执行中', completed: '已完成', failed: '失败', cancelled: '已停止', interrupted: '已中断' }[status] : '指令已接受';
}
export function handoffs(snapshot: RunSnapshot | null) {
  return snapshot ? resolveEdges(snapshot.events, snapshot.edges).filter(({ edge, source, target }) =>
    (edge.edgeType === 'handoff' || edge.edgeType === 'verification') && source.agent && target.agent && source.agent !== target.agent) : [];
}

export function canRetryConversation(snapshot: RunSnapshot | null, conversation: Conversation) {
  const turn = snapshot?.run.execution?.turns.find(t => t.promptId === conversation.id && t.agent === conversation.agent);
  return !!turn && conversations(snapshot?.events ?? [], snapshot?.edges ?? []).at(-1)?.id === conversation.id
    && turn.roleRevision === snapshot?.run.roles.revision
    && turn.logicalRole === (snapshot?.run.roles.analyst === turn.agent ? 'analyst' : 'implementer')
    && !['submitted', 'running'].includes(turn.status);
}

const ConversationCard = memo(function ConversationCard({ conversation, latest, snapshot, canSend, send, enabled, respond, links, retry, canRetry }: {
  retry: (agent: AgentId, promptId: string) => Promise<boolean>; canRetry: boolean;
  links: ReturnType<typeof handoffs>;
  enabled: boolean; respond?: RespondInteraction;
  conversation: Conversation; latest: boolean; snapshot: RunSnapshot | null; canSend: boolean;
  send: (agent: AgentId, content: string, sourceId?: string, requirements?: string, continuationId?: string) => Promise<boolean>;
}) {
  const [expanded, setExpanded] = useState<boolean>();
  const [requirements, setRequirements] = useState('');
  const [instruction, setInstruction] = useState('');
  const open = expanded ?? latest;
  const { agent, events, promptId } = conversation;
  const [project] = useState(displayProjection);
  const turn = snapshot?.run.execution?.turns.find(t => t.promptId === promptId);
  // Interaction audit entries belong in the audit view, not a second copy in
  // the conversation after the interactive control has disappeared.
  const visibleEvents = useMemo(() => project(events).filter(event => {
    if (event.kind !== 'system') return true;
    return !(turn?.interactions ?? []).some(request => request.kind?.startsWith('mcp_') && (
      event.content === `等待用户处理：${request.title}`
      || event.content === request.title
      || event.content.startsWith(`${request.title}：选择 `) && event.content.endsWith('（未保存表单回答）。')
    ));
  }), [project, events, turn?.interactions]);
  const settled = !turn || !['submitted', 'running'].includes(turn.status);
  const body = useAutoFollow(open, events, !settled);
  const other = agent === 'codex' ? 'claude' : 'codex';
  const lastMessage = [...visibleEvents].reverse().find(event => event.kind === 'message');
  const preview = lastMessage ?? events.find(event => event.kind === 'prompt');
  const contentId = `conversation-content-${conversation.id}`;
  return <li className={`conversation-row ${agent}`}>
    <div className="conversation-frame">
    <article className="conversation-card" aria-label={`${names[agent]} 对话`}>
      <header className="conversation-heading">
        <strong className={`conversation-agent ${agent}`}>{names[agent]}</strong>
        <span>{historicalRole(turn?.logicalRole ? turn : events[0])}</span>
        <span className="muted">{promptId ? deliveryStatus(snapshot, promptId) : '历史记录'}</span>
        <time dateTime={conversation.createdAt}>任务时间：{formatTime(conversation.createdAt)}</time>
        <button type="button" aria-expanded={open} aria-controls={contentId} onClick={() => setExpanded(!open)}>{open ? '收起内容' : '展开内容'}</button>
      </header>
      <TurnProgress turn={turn} conversation={conversation} visibleEvents={visibleEvents} open={open} active={!!snapshot?.run.execution?.chainActive} enabled={enabled} respond={respond} />
      {turn?.roleSkill && <details><summary>本轮角色技能来源</summary><p>{turn.roleSkill.path}</p><pre>{turn.roleSkill.content}</pre></details>}
      {links.filter(link => link.target.id === promptId).map(link => <p className="muted" key={link.edge.id}>{link.edge.edgeType === 'verification' ? '遇阻查证' : '角色交接'}：{names[link.source.agent!]}（{historicalRole(link.source)}） → {names[link.target.agent!]}（{historicalRole(link.target)}）</p>)}
      {!open && <p className="conversation-preview">{preview ? eventContent(preview).slice(0, 180) : `${events.length} 条执行记录`}</p>}
      <div ref={body} id={contentId} hidden={!open} className="conversation-body" tabIndex={0} aria-label="对话内容，自动跟踪最新信息">
        {visibleEvents.map((event, index) => <EventRow key={index} event={event} settled={!!(event.kind === 'prompt' || event.complete || settled)} />)}
        {lastMessage && !links.some(link => link.source.id === lastMessage.id) && <>
          {(lastMessage.content.includes('"handoff.v1"') || lastMessage.content.includes('"handoff_type"')) && <p className="muted">交接请求尚未派发；请查看执行状态或恢复检查点。</p>}
          <div className="handoff-controls">
            <button type="button" className="handoff-button" disabled={!canSend} onClick={() => void send(other, lastMessage.content, lastMessage.id, requirements.trim() ? requirements : undefined)}>交给 {names[other]}</button>
            <input aria-label={`交给 ${names[other]} 的补充要求`} placeholder="补充要求（可选）" value={requirements} maxLength={16000} disabled={!canSend} onChange={event => setRequirements(event.target.value)} />
          </div>
        </>}
        <form className="handoff-controls" aria-label={`继续执行 ${names[agent]} 当前任务`} onSubmit={event => {
          event.preventDefault();
          const text = instruction;
          void send(agent, text, undefined, undefined, promptId ?? conversation.id).then(ok => {
            if (ok) setInstruction(current => current === text ? '' : current);
          });
        }}>
          <input aria-label={`给 ${names[agent]} 的最新指示`} placeholder="输入最新指示，继续当前任务（不移交）" value={instruction} maxLength={16000} onChange={event => setInstruction(event.target.value)} />
          <button type="submit" disabled={!canSend || !instruction.trim()}>继续执行 · {names[agent]}</button>
        </form>
      </div>
    </article>
    {latest && <div className="conversation-footer">
      <button type="button" className="conversation-retry" aria-label={`重试 ${names[agent]} 当前会话`} title="重试：清空此方框内的旧履历，重新执行原始提示词；不会自动交接。可能再次修改工作区文件。" disabled={!canRetry || !canRetryConversation(snapshot, conversation)} onClick={() => { setExpanded(true); void retry(agent, conversation.id); }}>
        <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false"><path d="M21 3v6h-6" /><path d="M21 9l-3.3-3.3a8 8 0 1 0 1.9 8.3" /></svg>
      </button>
    </div>}
    </div>
  </li>;
}, (a, b) => a.conversation === b.conversation && a.latest === b.latest && a.canSend === b.canSend
  && a.enabled === b.enabled && a.respond === b.respond && a.canRetry === b.canRetry && a.retry === b.retry
  && (a.send === b.send || !b.canSend)
  && a.snapshot?.run.execution?.chainActive === b.snapshot?.run.execution?.chainActive
  && JSON.stringify(a.snapshot?.run.execution?.turns.find(t => t.promptId === a.conversation.promptId)) === JSON.stringify(b.snapshot?.run.execution?.turns.find(t => t.promptId === b.conversation.promptId))
  && a.links === b.links);

export default function AgentWorkspace(props: WorkspaceProps & { live?: ReturnType<typeof liveSnapshot> }) {
  const snapshot = useSyncExternalStore(props.live?.subscribe ?? (() => () => {}), props.live?.get ?? (() => props.snapshot), () => props.snapshot);
  return <WorkspaceContent {...props} snapshot={snapshot} />;
}

interface WorkspaceProps {
  onRespond?: RespondInteraction;
  snapshot: RunSnapshot | null; enabled: boolean;
  onSend: (agent: AgentId, content: string, sourceId?: string, autoHandoff?: boolean, requirements?: string, continuationId?: string) => Promise<boolean>;
  onStop: () => Promise<void>;
  onRecover?: () => Promise<boolean>;
  onReset?: (agent: AgentId) => Promise<boolean>;
  onRestart?: (agent: AgentId, promptId: string) => Promise<boolean>;
}

function WorkspaceContent({ snapshot, enabled, onSend, onStop, onReset, onRestart, onRecover, onRespond }: WorkspaceProps) {
  const [selected, setSelected] = useState<AgentId>('codex');
  const retryTurn = snapshot?.run.execution?.turns.filter(turn => turn.agent === selected).at(-1);
  const retryable = retryTurn && retryTurn.roleRevision === snapshot?.run.roles.revision && (retryTurn.status !== 'completed' || retryTurn.interactions?.some(request => request.status === 'blocked'));
  const [drafts, setDrafts] = useState({ codex: '', claude: '' });
  const [sending, setSending] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [automatic, setAutomatic] = useState(false);
  const [confirmReset, setConfirmReset] = useState(false);
  const locked = useRef(false);
  const [project] = useState(conversationProjection);
  const timeline = useMemo(() => project(snapshot?.events ?? [], snapshot?.edges ?? []), [project, snapshot?.events, snapshot?.edges]);
  const linkCache = useRef({ key: '', links: [] as ReturnType<typeof handoffs> });
  const linkKey = `${snapshot?.run.id}:${snapshot?.run.execution?.historyRevision ?? 0}:${snapshot?.edges.filter(e => e.edgeType === 'handoff' || e.edgeType === 'verification').map(e => e.id).join(',')}`;
  if (linkCache.current.key !== linkKey) linkCache.current = { key: linkKey, links: handoffs(snapshot) };
  const running = !!snapshot && (!!snapshot.run.execution?.chainActive || ['analyzing', 'implementing', 'verifying', 'updating_skill'].includes(snapshot.run.state));
  const canSend = enabled && snapshot?.run.state === 'ready' && !running && !sending;
  const canRetry = !!onRestart && enabled && !running && !sending && !stopping && !!snapshot && ['ready', 'paused', 'interrupted'].includes(snapshot.run.state);
  const retry = useCallback(async (agent: AgentId, promptId: string) => {
    if (!canRetry || locked.current) return false;
    locked.current = true; setSending(true);
    try { return await onRestart!(agent, promptId); }
    finally { locked.current = false; setSending(false); }
  }, [canRetry, onRestart]);
  useEffect(() => { if (!running) setStopping(false); }, [running]);
  const send = async (agent: AgentId, text: string, sourceId?: string, requirements?: string, continuationId?: string) => {
    if (!canSend || locked.current || !text.trim()) return false;
    locked.current = true; setSending(true);
    try {
      if (await onSend(agent, text, sourceId, continuationId ? false : automatic, requirements, continuationId)) {
        if (!continuationId) setAutomatic(false);
        if (!sourceId && !continuationId) setDrafts(current => current[agent] === text ? { ...current, [agent]: '' } : current);
        return true;
      }
      return false;
    } finally { locked.current = false; setSending(false); }
  };
  return <section aria-label="智能体工作区">
    <section className="panel conversation-timeline" aria-label="协作时间轴">
      <div className="panel-heading"><h3>协作时间轴</h3><span className="muted">Codex 在左 · Claude 在右 · 按执行顺序排列</span></div>
      {timeline.length ? <ol className="conversation-list">{timeline.map((conversation, index) => <ConversationCard retry={retry} canRetry={canRetry} links={linkCache.current.links} key={`${snapshot?.run.id}-${conversation.id}`} conversation={conversation} latest={index === timeline.length - 1} snapshot={snapshot} canSend={canSend} send={send} enabled={enabled && !stopping} respond={onRespond} />)}</ol> : <p className="empty">暂无对话。在下方选择 Codex 或 Claude，输入提示词开始任务。</p>}
    </section>
    <form className="panel prompt-composer" onSubmit={event => { event.preventDefault(); void send(selected, drafts[selected]); }}>
      <div className="panel-heading composer-heading"><label htmlFor="agent-prompt">提示词 · 发送给 {names[selected]}</label><div className="recipient-select"><label htmlFor="prompt-recipient">接收方</label><select id="prompt-recipient" value={selected} disabled={sending} onChange={event => { setSelected(event.target.value as AgentId); setConfirmReset(false); }}><option value="codex">Codex</option><option value="claude">Claude</option></select></div></div>
      <p className="muted">两侧草稿独立保存，交换角色不改变接收方。{snapshot?.run.execution?.[selected === 'codex' ? 'codexSession' : 'claudeSession'] ? '已保存此侧会话，下一条指令将接续上下文。' : '此侧尚无会话，下一条指令将启动新会话。'}</p>
      {snapshot && <p className="muted">当前接收方职责：{snapshot.run.roles.analyst === selected ? '分析方（REA 查证，不编写功能源码）' : '实现方（依据技能编写，遇阻停写并请求查证）'}。每轮从目标工作区加载 .rule/{snapshot.run.roles.analyst === selected ? 'analyst' : 'implementer'}/SKILL.md；交换角色后启动新会话，保留任务技能和历史记录。</p>}
      <label className="auto-handoff"><input type="checkbox" checked={automatic} disabled={!enabled || running || sending} onChange={event => setAutomatic(event.target.checked)} />允许本次指令自动交接给对方（最多 4 次）</label>
      <p className="muted">仅处理成功完成的最终回复中的有效交接请求。启用后可能连续调用模型并修改工作区；停止将取消后续派发。</p>
      <textarea id="agent-prompt" rows={4} value={drafts[selected]} maxLength={16000} onChange={event => setDrafts(current => ({ ...current, [selected]: event.target.value }))} placeholder={`输入给 ${names[selected]} 的提示词…`} />
      <div className="actions"><button className="primary" disabled={!canSend || !drafts[selected].trim()}>{sending ? '正在提交…' : `发送给 ${names[selected]}`}</button>
        <button type="button" disabled={!running || stopping} onClick={() => { setStopping(true); void onStop().catch(() => setStopping(false)); }}>{stopping ? '正在停止并清理进程…' : '停止当前执行'}</button>
      </div>
      {onReset && <div className="session-reset"><button type="button" disabled={!enabled || running} onClick={() => setConfirmReset(!confirmReset)}>重置所选智能体会话…</button>{confirmReset && <><p>确认清除 {names[selected]} 的会话接续标识？历史消息保留，下一条指令将不再接续旧上下文。</p><button type="button" disabled={!enabled || running} onClick={() => { void onReset(selected).then(ok => { if (ok) setConfirmReset(false); }); }}>确认重置 {names[selected]}</button><button type="button" onClick={() => setConfirmReset(false)}>取消</button></>}</div>}
      {onRestart && retryable && timeline.at(-1)?.promptId === retryTurn.promptId && <div className="session-reset"><p>已检查已有输出或调整读取方式后，可重启 {names[selected]} 并重新投递最后一个会话的问题包。另一侧会话、待查问题和历史保留，本次自动交接关闭。正在执行时请先停止并等待清理。</p><button type="button" disabled={!canRetry} onClick={() => {
        if (locked.current) return;
        locked.current = true; setSending(true);
        void onRestart(selected, retryTurn.promptId).finally(() => { locked.current = false; setSending(false); });
      }}>仅重启此侧并重试 · {names[selected]}</button></div>}
      {snapshot && ['paused', 'interrupted'].includes(snapshot.run.state) && <div className="notice" role="status">
        <div><strong>{running ? '正在结束执行链，请稍候…' : '执行已暂停，确认后可继续发送指令'}</strong>
          <p>{snapshot.checkpoint.nextStep}</p>
          <p>请先检查已有输出及文件修改。恢复会保留两侧会话和当前草稿，不会重放旧任务或自动交接；确认后可发送“继续完成剩余任务”或新的指令。</p>
          {onRecover ? <button type="button" disabled={!enabled || running || sending || stopping} onClick={() => { void onRecover(); }}>确认检查点并恢复输入</button> : <p>请在“恢复与技能”中确认检查点后继续。</p>}
        </div>
      </div>}
    </form>
  </section>;
}