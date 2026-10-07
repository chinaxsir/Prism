/** 节点延迟展示逻辑（抽离自 Proxies 页，便于单测） */

/** 0 = 超时；undefined = 未测速 */
export type Delay = number | undefined;

/** 延迟对应的状态色 class */
export function delayClass(delay: Delay): string {
  if (delay === undefined) return "text-gray-500";
  if (delay === 0) return "text-latency-bad";
  if (delay < 200) return "text-latency-good";
  if (delay < 500) return "text-latency-medium";
  return "text-latency-bad";
}

/** 延迟的人类可读文本 */
export function delayText(delay: Delay): string {
  if (delay === undefined) return "未测速";
  if (delay === 0) return "超时";
  return `${delay} ms`;
}
