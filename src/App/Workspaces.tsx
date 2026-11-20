import { useState } from 'react';
import type { Run } from '../types';
import { displayWorkspacePath } from './workspace-path';

export default function Workspaces({ items, selected, enabled, manage }: {
  items: Run[]; selected?: string; enabled: boolean;
  manage: (operation: string, path?: string) => Promise<boolean>;
}) {
  const [path, setPath] = useState('');
  const [adding, setAdding] = useState(false);
  const [deleting, setDeleting] = useState(false);
  return <section className="workspace-picker" aria-label="工作区列表">
    <h3>工作区</h3>
    {items.map(item => <button key={item.id} className={`workspace-choice ${selected === item.id ? 'active' : ''}`} disabled={!enabled} aria-pressed={selected === item.id} title={displayWorkspacePath(item.workspace)} onClick={() => { setDeleting(false); void manage('select', item.id); }}>{displayWorkspacePath(item.workspace)}</button>)}
    {!items.length && <p>暂无工作区，请添加项目目录。</p>}
    <div className="actions"><button disabled={!enabled} onClick={() => setAdding(!adding)}>添加工作区…</button><button disabled={!enabled || !selected} onClick={() => setDeleting(!deleting)}>移除工作区…</button></div>
    {adding && <form onSubmit={e => { e.preventDefault(); void manage('add', path).then(ok => { if (ok) { setPath(''); setAdding(false); } }); }}>
      <label htmlFor="new-workspace">新工作区绝对路径</label><input id="new-workspace" value={path} disabled={!enabled} onChange={e => setPath(e.target.value)} />
      <button disabled={!enabled || !path.trim()}>添加</button>
    </form>}
    {deleting && <div role="alertdialog" aria-label="确认移除工作区">
      <p>移除当前工作区及其本应用中的任务记录，无法撤销。磁盘上的项目文件、已导出的技能文件和 Codex／Claude 自身历史记录不会删除。</p>
      <button disabled={!enabled} onClick={() => void manage('delete').then(ok => { if (ok) setDeleting(false); })}>确认移除工作区</button><button onClick={() => setDeleting(false)}>取消</button>
    </div>}
  </section>;
}