import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { Bootstrap, EnvCheckReport, RunSnapshot, Run, WorkspaceCatalog } from '../types';
import { newerSnapshot } from '../view-model';
import { applyRunUpdate, frameQueue, type RunUpdate } from '../stream-updates';
import { CheckpointView, Diagnostics, RoleCard, SkillHistory } from './panels';
import AgentWorkspace from './AgentWorkspace';
import DeleteHistory from './DeleteHistory';
import Workspaces from './Workspaces';
import { displayWorkspacePath } from './workspace-path';
import { controlsChanged, liveSnapshot } from '../live-snapshot';

type View = 'workspace' | 'environment' | 'recovery';
const desktop = isTauri();

export default function App() {
  const [view, setView] = useState<View>('workspace');
  const [snapshot, setSnapshot] = useState<RunSnapshot | null>(null);
  const snapshotRef = useRef<RunSnapshot | null>(null);
  const [live] = useState(liveSnapshot);
  const [environment, setEnvironment] = useState<EnvCheckReport | null>(null);
  const [dataDirectory, setDataDirectory] = useState('');
  const [busy, setBusy] = useState(false);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [workspace, setWorkspace] = useState('');
  const [workspaces, setWorkspaces] = useState<Run[]>([]);
  const activeRun = useRef<string | null>(null);
  const started = useRef(false);
  const diagnosticsBusy = useRef(false);
  const operationBusy = useRef(false);
  const merge = useCallback((next: RunSnapshot) => {
    if (next.run.id !== activeRun.current) return;
    snapshotRef.current = newerSnapshot(snapshotRef.current, next);
    live.set(snapshotRef.current);
    setSnapshot(snapshotRef.current);
    setWorkspaces(current => current.map(item => item.id === next.run.id ? next.run : item));
  }, [live]);
  const selectCatalog = useCallback((value: WorkspaceCatalog) => {
    activeRun.current = value.snapshot?.run.id ?? null;
    snapshotRef.current = value.snapshot;
    live.set(value.snapshot);
    setSnapshot(value.snapshot); setWorkspaces(value.workspaces);
    setWorkspace(displayWorkspacePath(value.snapshot?.run.workspace ?? '')); setEnvironment(null);
  }, [live]);

  const diagnose = useCallback(async (runId: string) => {
    if (diagnosticsBusy.current) return;
    diagnosticsBusy.current = true;
    setChecking(true);
    try {
      const report = await invoke<EnvCheckReport>('inspect_environment', { runId });
      if (activeRun.current === runId) setEnvironment(report);
    } catch {
      setError('环境诊断失败，请检查应用权限后重试。');
    } finally { diagnosticsBusy.current = false; setChecking(false); }
  }, []);

  const load = useCallback(async () => {
    if (!desktop) return;
    setBusy(true); setError(null);
    try {
      const value = await invoke<Bootstrap>('bootstrap');
      setDataDirectory(value.dataDirectory);
      selectCatalog(value);
      if (value.snapshot) {
        void diagnose(value.snapshot.run.id);
      }
    } catch { setError('无法初始化本地存储，请检查应用数据目录的写入权限后重试。'); }
    finally { setBusy(false); }
  }, [diagnose, selectCatalog]);

  useEffect(() => { if (!started.current) { started.current = true; void load(); } }, [load]);
  useEffect(() => {
    const preventMenu = (event: MouseEvent) => event.preventDefault();
    document.addEventListener('contextmenu', preventMenu);
    return () => document.removeEventListener('contextmenu', preventMenu);
  }, []);
  useEffect(() => {
    if (!desktop) return;
    let stopped = false;
    let off: (() => void) | undefined;
    let refreshing = false;
    const queue = frameQueue<RunUpdate>(updates => {
      const relevant = updates.filter(update => update.run.id === activeRun.current);
      if (!relevant.length) return;
      const current = snapshotRef.current;
      const next = relevant.reduce(applyRunUpdate, current);
      snapshotRef.current = next;
      live.set(next);
      if (controlsChanged(current, next)) setSnapshot(next);
      if (relevant.some(update => update.baseEventCount > (next?.events.length ?? 0)) && !refreshing) {
        refreshing = true;
        void invoke<RunSnapshot>('refresh_snapshot', { runId: activeRun.current })
          .then(value => { if (!stopped) merge(value); })
          .catch(() => { if (!stopped) setError('流式记录同步失败，请点击刷新。'); })
          .finally(() => { refreshing = false; });
      }
    });
    void listen<RunUpdate>('run-update', event => { if (!stopped) queue.push(event.payload); })
      .then(unsubscribe => { if (stopped) unsubscribe(); else off = unsubscribe; })
      .catch(() => { if (!stopped) setError('流式更新连接失败，请刷新后重试。'); });
    return () => { stopped = true; queue.dispose(); off?.(); };
  }, [merge, live]);
  useEffect(() => {
    if (!desktop) return;
    let stopped = false;
    let unsubscribe: (() => void) | undefined;
    void listen<RunSnapshot>('run-snapshot', event => { if (!stopped) merge(event.payload); })
      .then(off => { if (stopped) off(); else unsubscribe = off; })
      .catch(() => { if (!stopped) setError('实时更新连接失败，请点击“刷新”重新加载已保存的数据。'); });
    return () => { stopped = true; unsubscribe?.(); };
  }, [merge]);
  useEffect(() => {
    if (!desktop) return;
    let stopped = false;
    let off: (() => void) | undefined;
    void listen<string>('worker-error', event => { if (!stopped) setError(event.payload); })
      .then(unsubscribe => { if (stopped) unsubscribe(); else off = unsubscribe; })
      .catch(() => { if (!stopped) setError('工作进程错误通知连接失败，请刷新状态。'); });
    return () => { stopped = true; off?.(); };
  }, []);

  const action = async (command: string, args: Record<string, unknown> = {}): Promise<boolean> => {
    if (!snapshot || operationBusy.current) return false;
    operationBusy.current = true; setBusy(true); setError(null);
    try { merge(await invoke<RunSnapshot>(command, { runId: snapshot.run.id, ...args })); return true; }
    catch (error) { setError(command === 'submit_prompt' && typeof error === 'string' ? error : '操作失败，请先刷新已保存的状态，检查工作区路径和当前任务状态后重试。'); return false; }
    finally { operationBusy.current = false; setBusy(false); }
  };

  const state = snapshot?.run.state;
  const running = !!snapshot?.run.execution?.chainActive || ['analyzing', 'implementing', 'verifying', 'updating_skill'].includes(state ?? '');
  const manageWorkspace = async (operation: string, path?: string) => {
    if (operationBusy.current) return false;
    operationBusy.current = true; setBusy(true); setError(null);
    try {
      selectCatalog(await invoke<WorkspaceCatalog>('manage_workspace', { operation, runId: operation === 'select' ? path : snapshot?.run.id, workspace: operation === 'add' ? path : undefined, confirmed: operation === 'delete' }));
      return true;
    } catch (e) { setError(String(e)); return false; }
    finally { operationBusy.current = false; setBusy(false); }
  };
  const editable = desktop && !!snapshot && !busy && !checking;
  const canSwap = editable && (state === 'ready' || state === 'paused');
  const canDelete = editable && !snapshot?.run.execution?.chainActive && !['analyzing', 'implementing', 'verifying', 'updating_skill'].includes(state ?? '') && !snapshot?.run.execution?.turns.some(t => ['submitted', 'running'].includes(t.status));
  const analystReady = environment?.mcpServers.some(s => s.owner === snapshot?.run.roles.analyst && (s.connectionStatus || s.status) === 'ok' && s.toolChecks?.some(t => t.status === 'ok')) ?? false;

  return <div className="app-shell">
    <aside className="sidebar">
      <div className="brand"><span className="brand-mark">B</span><div><strong>BINWEAVE</strong><small>分析工作台</small></div></div>
      <Workspaces items={workspaces} selected={snapshot?.run.id} enabled={desktop && !busy && !running} manage={manageWorkspace} />
      <nav aria-label="工作区导航">
        {(['workspace', 'environment', 'recovery'] as View[]).map(item => <button key={item} className={view === item ? 'nav-item active' : 'nav-item'} aria-current={view === item ? 'page' : undefined} onClick={() => setView(item)}><span>{({ workspace: '◈', environment: '⌁', recovery: '↺' })[item]}</span>{({ workspace: '控制台', environment: '环境诊断', recovery: '恢复与技能' })[item]}</button>)}
      </nav>
      <footer><span className={desktop && snapshot ? 'bridge connected' : 'bridge'}>{desktop ? snapshot ? '已连接桌面存储' : '正在连接桌面存储' : '浏览器预览 · 无本地访问权限'}</span><small>v0.1.0 · 基础版</small></footer>
    </aside>
    <main>
      <header className="topbar"><div><span className="eyebrow">本地运行 · 操作员掌控</span><h1>{({ workspace: '控制台', environment: '环境诊断', recovery: '恢复与技能' })[view]}</h1></div><div className="actions"><button disabled={!editable} onClick={() => void action('refresh_snapshot')}>刷新</button><button disabled={!snapshot || checking || busy || !desktop} onClick={() => { if (snapshot) { setView('environment'); void diagnose(snapshot.run.id); } }}>{checking ? '正在检查…' : '运行诊断'}</button></div></header>
      {error && <div role="alert" className="notice error"><span>{error}</span><button onClick={() => setError(null)}>关闭提示</button>{!snapshot && desktop && <button disabled={busy} onClick={() => void load()}>重试</button>}</div>}
      {!desktop && <div className="notice">浏览器预览仅供查看。本地工具、已保存的会话和环境诊断需在 Tauri 桌面应用中使用。此处不会将示例事件显示为真实活动。</div>}
      <div className="notice subdued">对话按执行顺序显示在同一时间轴：Codex 在左，Claude 在右。卡片可展开或收起；在下方通过接收方下拉菜单发送提示词并接续各自会话。</div>
      {view === 'workspace' && <>
        <section className="mcp-policy-toolbar" aria-label="MCP 交互策略"><div className="mcp-policy-control"><label htmlFor="mcp-policy">MCP 交互策略</label><select id="mcp-policy" aria-describedby="mcp-policy-help" disabled={!editable || running} value={snapshot?.run.execution?.elicitationPolicy ?? 'ask'} onChange={event => void action('set_elicitation_policy', { policy: event.target.value })}><option value="ask">询问用户（默认）</option><option value="decline">无人值守：拒绝请求</option><option value="cancel">无人值守：取消请求</option></select></div><details className="mcp-policy-help"><summary>策略说明</summary><p id="mcp-policy-help" className="muted">策略保存到当前工作区，仅影响后续轮次。拒绝或取消会停止自动交接并保留人工恢复检查点；不会自动接受表单或打开链接。</p></details></section>
        <section className="panel"><div className="panel-heading"><h3>工作区路径</h3><span className="muted">任务 {snapshot?.run.id.slice(0, 8) ?? '未初始化'}</span></div><form className="inline-form" onSubmit={event => { event.preventDefault(); void action('set_workspace', { workspace }).then(ok => { if (ok) setEnvironment(null); }); }}><label htmlFor="workspace">绝对路径</label><input id="workspace" value={workspace} placeholder="请输入已存在的工作区绝对路径" disabled={!editable || running} onChange={event => setWorkspace(event.target.value)} /><button type="button" disabled={!editable || running} onClick={() => setWorkspace('')}>清空路径</button><button disabled={!editable || running || !workspace.trim() || workspace === displayWorkspacePath(snapshot?.run.workspace ?? '')}>保存工作区</button></form><p className="muted">路径可编辑或清空后重新填写；保存会断开旧 Agent 会话。移除整个工作区请使用左侧按钮。</p></section>
        <section className="roles"><RoleCard role="analyst" agent={snapshot?.run.roles.analyst ?? 'codex'} ready={analystReady} /><button className="swap" disabled={!canSwap} onClick={() => void action('swap_roles')} title="交换角色并生成新版本的技能契约">⇄<small>交换角色</small></button><RoleCard role="implementer" agent={snapshot?.run.roles.implementer ?? 'claude'} ready={false} /></section>
        {snapshot && !analystReady && <div className="notice">当前分析员尚无已验证的 REA MCP 连接，请在启用分析进程前检查环境诊断。</div>}
        <AgentWorkspace live={live} key={snapshot?.run.id ?? 'empty'} snapshot={snapshot} enabled={editable} onRespond={async (promptId, requestId, answer) => { if (!snapshot || !desktop) return false; merge(await invoke<RunSnapshot>('respond_interaction', { runId: snapshot.run.id, promptId, requestId, answer })); return true; }} onSend={(agent, content, sourceId, autoHandoff, requirements, continuationId) => action('submit_prompt', { agent, content, sourceId, autoHandoff, ...(requirements === undefined ? {} : { requirements }), ...(continuationId === undefined ? {} : { continuationId }) })} onRestart={(agent, restartId) => action('submit_prompt', { agent, restartId, content: '单侧恢复', autoHandoff: false })} onReset={agent => action('reset_agent_session', { agent })} onRecover={() => action('acknowledge_recovery')} onStop={async () => { try { await invoke('cancel_prompt', { runId: snapshot?.run.id }); } catch (e) { setError(`停止失败：${String(e)}`); throw e; } }} />
      </>}
      {view === 'workspace' && <DeleteHistory enabled={canDelete} onDelete={() => action('delete_history', { confirmed: true })} />}
      {view === 'environment' && <Diagnostics report={environment} checking={checking} />}
      {view === 'recovery' && <><CheckpointView snapshot={snapshot} directory={dataDirectory} disabled={!editable} onAcknowledge={() => void action('acknowledge_recovery')} /><SkillHistory artifacts={snapshot?.artifacts ?? []} /></>}
    </main>
  </div>;
}