/** 时间相关纯函数（抽离自 Subscription 页，便于单测） */

/**
 * 兼容秒/毫秒时间戳，输出相对时间。
 * 未来时间直接回退为本地时间字符串。
 */
export function formatRelativeTime(ts: number): string {
  if (!ts) return "从未更新";
  const ms = ts < 1e12 ? ts * 1000 : ts;
  const diff = Date.now() - ms;
  if (diff < 0) return new Date(ms).toLocaleString("zh-CN");
  const minutes = Math.floor(diff / 60000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days} 天前`;
  return new Date(ms).toLocaleDateString("zh-CN");
}
