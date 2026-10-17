/**
 * 开关组件：36×20 轨道 + 16px 滑块，flex + padding 定位
 * （checked: 2px 内边距 + translate-x-4=16px → 右缘 34px，两侧各留 2px，
 * 不会溢出轨道；取代此前 absolute 定位 + 大位移的写法）
 */
export default function Switch({
  checked,
  onChange,
  title,
}: {
  checked: boolean;
  onChange?: (checked: boolean) => void;
  title?: string;
}) {
  return (
    <button
      type="button"
      title={title}
      onClick={() => onChange?.(!checked)}
      className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full p-0.5 transition-colors duration-200 ${
        checked ? "bg-accent" : "bg-white/15"
      }`}
    >
      <span
        className={`pointer-events-none inline-block h-4 w-4 rounded-full bg-white shadow-sm transition-transform duration-200 ${
          checked ? "translate-x-4" : "translate-x-0"
        }`}
      />
    </button>
  );
}
