import { useState, type ReactNode } from "react";
import { Lock } from "lucide-react";

import { FEATURE_LABEL, isMobile, type Feature } from "@/pro/gating";
import { useProStore } from "@/stores/pro";
import { toast } from "@/components/ui/Toast";

/// 激活 Pro 弹窗（占位校验，可复用）
export function ActivateProModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const activate = useProStore((s) => s.activate);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);

  if (!open) return null;

  const submit = async () => {
    if (busy || !code.trim()) return;
    setBusy(true);
    // TODO(商业化)：替换为真实内购（StoreKit / Google Play Billing）
    const ok = await activate(code.trim());
    setBusy(false);
    if (ok) {
      toast.success("Pro 已激活，高级功能全部解锁");
      setCode("");
      onClose();
    } else {
      toast.error("激活码无效");
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <div
        className="w-full max-w-sm space-y-4 rounded-xl bg-surface-card p-6"
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="text-base font-semibold">激活 Pro</h3>
        <p className="text-xs text-gray-400">
          输入激活码解锁全部高级功能。当前为占位校验，格式
          <span className="font-mono text-accent"> PRISM-XXXX-XXXX-XXXX </span>
          即可通过（测试用）。
        </p>
        <input
          type="text"
          value={code}
          onChange={(e) => setCode(e.target.value)}
          placeholder="PRISM-XXXX-XXXX-XXXX"
          className="w-full rounded-lg border border-white/5 bg-surface-hover px-4 py-2 font-mono text-sm outline-none focus:border-accent"
        />
        <div className="flex justify-end gap-2">
          <button
            onClick={onClose}
            className="rounded-lg px-4 py-2 text-sm text-gray-400 hover:text-gray-200"
          >
            取消
          </button>
          <button
            onClick={submit}
            disabled={busy || !code.trim()}
            className="rounded-lg bg-accent px-5 py-2 text-sm transition-colors hover:bg-accent-hover disabled:opacity-50"
          >
            {busy ? "激活中…" : "激活"}
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * Pro 门控：移动端未解锁时给 children 盖毛玻璃遮罩 + 激活入口；
 * 桌面端或已解锁时完全透明穿透（无任何视觉差异）。
 */
export default function ProGate({
  feature,
  children,
}: {
  feature: Feature;
  children: ReactNode;
}) {
  const unlocked = useProStore((s) => s.unlocked);
  const [showActivate, setShowActivate] = useState(false);

  if (!isMobile() || unlocked) return <>{children}</>;

  return (
    <div className="relative">
      <div className="pointer-events-none select-none" aria-hidden>
        {children}
      </div>
      <div className="absolute inset-0 flex items-center justify-center bg-surface/60 backdrop-blur-sm">
        <div className="space-y-3 p-6 text-center">
          <Lock size={28} className="mx-auto text-accent" />
          <p className="text-sm text-gray-300">
            {FEATURE_LABEL[feature]}为 Pro 功能
          </p>
          <button
            onClick={() => setShowActivate(true)}
            className="rounded-lg bg-accent px-5 py-2 text-sm transition-colors hover:bg-accent-hover"
          >
            激活 Pro
          </button>
        </div>
      </div>
      <ActivateProModal
        open={showActivate}
        onClose={() => setShowActivate(false)}
      />
    </div>
  );
}
