import { Minus, Square, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";

/// 无边框窗口的自定义标题栏：可拖动区域 + 最小化/最大化/关闭
export default function TitleBar() {
  const appWindow = getCurrentWindow();

  return (
    <div
      data-tauri-drag-region
      className="hidden md:flex h-9 shrink-0 items-center justify-between pl-4 pr-2 bg-surface border-b border-white/5 select-none"
    >
      <span
        data-tauri-drag-region
        className="text-xs text-gray-500 font-medium tracking-wide"
      >
        Prism Proxy
      </span>

      <div className="flex items-center gap-1">
        <button
          onClick={() => appWindow.minimize()}
          className="p-1.5 rounded-md hover:bg-surface-hover text-gray-400 hover:text-gray-200 transition-colors"
          title="最小化"
        >
          <Minus size={14} />
        </button>
        <button
          onClick={() => appWindow.toggleMaximize()}
          className="p-1.5 rounded-md hover:bg-surface-hover text-gray-400 hover:text-gray-200 transition-colors"
          title="最大化"
        >
          <Square size={12} />
        </button>
        <button
          onClick={() => appWindow.close()}
          className="p-1.5 rounded-md hover:bg-latency-bad/80 text-gray-400 hover:text-white transition-colors"
          title="隐藏到托盘（托盘菜单可退出）"
        >
          <X size={14} />
        </button>
      </div>
    </div>
  );
}
