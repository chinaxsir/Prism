import clsx from "clsx";

/**
 * 统一的功能开关（Settings / Subscription 共用）：
 * 44×24 轨道 + 20px 滑块，主题主色（accent）激活态。
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
        "relative w-11 h-6 rounded-full transition-colors shrink-0 disabled:opacity-50",
        checked ? "bg-accent" : "bg-white/10"
      )}
    >
      <span
        className={clsx(
          "absolute top-0.5 w-5 h-5 rounded-full bg-white shadow transition-transform",
          checked ? "translate-x-[22px]" : "translate-x-0.5"
        )}
      />
    </button>
  );
}
