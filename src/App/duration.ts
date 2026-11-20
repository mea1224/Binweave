export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds)) return '未知';
  const total = Math.max(0, Math.floor(seconds));
  if (total < 60) return `${total} 秒`;
  if (total < 3600) return `${Math.floor(total / 60)} 分 ${total % 60} 秒`;
  return `${Math.floor(total / 3600)} 小时 ${Math.floor(total % 3600 / 60)} 分`;
}