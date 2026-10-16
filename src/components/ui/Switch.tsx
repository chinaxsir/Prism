import clsx from "clsx";

/**
 * 统一的功能开关（Settings / Subscription 共用）：
 * 紧凑 36×20 轨道 + 16px 滑块，主题主色（accent）激活态。
 * 较小尺寸降低窄屏（移动端 WebView）下行内溢出的风险，
 * 行容器另有 overflow-hidden 双重兜底。
 */
export default function Switch({
  checked,
  onChange,
  disabled,
  title,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  title?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      title={title}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={clsx(
        "relative w-9 h-5 rounded-full transition-colors shrink-0 disabled:opacity-50",
        checked ? "bg-accent" : "bg-white/10"
      )}
    >
      <span
        className={clsx(
          "absolute top-0.5 w-4 h-4 rounded-full bg-white shadow transition-transform",
          checked ? "translate-x-[18px]" : "translate-x-0.5"
        )}
      />
    </button>
  );
}
