import { useRef, useState } from 'react';

export const deletionWarning = '此操作将删除当前任务在本应用中的对话、工具输出、操作笔记和轮次记录，无法撤销。Codex／Claude 自身的历史记录仍旧存在，不会被删除。本应用会断开两侧会话接续；工作区文件和技能版本保留。';

export default function DeleteHistory({ enabled, onDelete }: { enabled: boolean; onDelete: () => Promise<boolean> }) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const locked = useRef(false);
  const remove = async () => {
    if (!enabled || locked.current) return;
    locked.current = true; setBusy(true); setError('');
    try {
      if (await onDelete()) setConfirming(false);
      else setError('删除未完成，请检查错误提示后重试。');
    } catch (e) { setError(`删除失败：${String(e)}`); }
    finally { locked.current = false; setBusy(false); }
  };
  return <section className="panel history-deletion" aria-label="删除本地记录">
    <button disabled={!enabled || busy} onClick={() => { setConfirming(true); setError(''); }}>删除记录…</button>
    <p className="muted">仅删除本应用中当前任务的记录；执行期间不可删除。</p>
    {confirming && <div role="alertdialog" aria-labelledby="delete-history-title" aria-describedby="delete-history-warning">
      <h3 id="delete-history-title">确认删除本地记录？</h3>
      <p id="delete-history-warning">{deletionWarning}</p>
      <div className="actions"><button disabled={!enabled || busy} onClick={() => void remove()}>{busy ? '正在删除…' : '确认删除记录'}</button><button disabled={busy} onClick={() => setConfirming(false)}>取消</button></div>
      {error && <p role="alert">{error}</p>}
    </div>}
  </section>;
}