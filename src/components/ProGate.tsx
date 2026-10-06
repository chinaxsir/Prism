import { useEffect, useState, type ReactNode } from "react";
import { Loader2, Lock, Sparkles } from "lucide-react";

import { FEATURE_LABEL, isMobile, type Feature } from "@/pro/gating";
import {
  STORE_PRODUCTS,
  buyProduct,
  fetchStoreProducts,
  restoreAll,
} from "@/pro/iap";
import { useProStore } from "@/stores/pro";
import type { Product } from "@choochmeque/tauri-plugin-iap-api";
import { toast } from "@/components/ui/Toast";

/// 激活 Pro 弹窗：激活码（全平台）+ 商店内购（仅移动端）
export function ActivateProModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const activate = useProStore((s) => s.activate);
  const error = useProStore((s) => s.error);
  const [code, setCode] = useState("");
  const [email, setEmail] = useState("");
  const [busy, setBusy] = useState(false);
  const [products, setProducts] = useState<Product[]>([]);
  const mobile = isMobile();

  useEffect(() => {
    if (!open || !mobile) return;
    fetchStoreProducts()
      .then(setProducts)
      .catch((e) => console.warn("fetch products failed:", e));
  }, [open, mobile]);

  if (!open) return null;

  const emailOk = /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email.trim());

  // 激活码激活（邮箱+授权码）
  const submitCode = async () => {
    if (busy || !code.trim() || !emailOk) return;
    setBusy(true);
    const ok = await activate(code.trim(), email.trim());
    setBusy(false);
    if (ok) {
      toast.success("Pro 已激活，高级功能全部解锁");
      setCode("");
      setEmail("");
      onClose();
    } else {
      toast.error(error ?? "激活失败，请检查邮箱与激活码");
    }
  };

  // 移动端商店购买
  const buy = async (productId: string, type: "inapp" | "subs") => {
    if (busy) return;
    setBusy(true);
    try {
      await buyProduct(productId, type);
      toast.success("购买成功，Pro 已激活");
      onClose();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  // 恢复购买（换机 / 重装）
  const restore = async () => {
    if (busy) return;
    setBusy(true);
    try {
      const count = await restoreAll();
      if (count > 0) {
        toast.success(`已恢复 ${count} 笔购买`);
        onClose();
      } else {
        toast.info("未找到可恢复的购买");
      }
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const priceOf = (id: string) =>
    products.find((p) => p.productId === id)?.formattedPrice;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <div
        className="max-h-[90vh] w-full max-w-sm space-y-5 overflow-y-auto rounded-xl bg-surface-card p-6"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2">
          <Sparkles size={18} className="text-accent" />
          <h3 className="text-base font-semibold">激活 Prism Pro</h3>
        </div>

        {/* 移动端：商店内购 */}
        {mobile && (
          <div className="space-y-2">
            {STORE_PRODUCTS.map((p) => (
              <button
                key={p.id}
                onClick={() => buy(p.id, p.type)}
                disabled={busy}
                className="flex w-full items-center justify-between rounded-lg border border-white/5 bg-surface-hover px-4 py-3 text-sm transition-colors hover:border-accent/50 disabled:opacity-50"
              >
                <span>{p.label}</span>
                <span className="font-medium text-accent">
                  {priceOf(p.id) ?? "…"}
                </span>
              </button>
            ))}
            <button
              onClick={restore}
              disabled={busy}
              className="w-full rounded-lg px-4 py-2 text-xs text-gray-400 hover:text-gray-200 disabled:opacity-50"
            >
              恢复购买
            </button>
          </div>
        )}

        {/* 分隔线（移动端才显示） */}
        {mobile && (
          <div className="flex items-center gap-3 text-xs text-gray-500">
            <div className="h-px flex-1 bg-white/5" />
            或使用激活码
            <div className="h-px flex-1 bg-white/5" />
          </div>
        )}

        {/* 邮箱 + 激活码 */}
        <div className="space-y-3">
          <div className="space-y-1">
            <input
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="you@example.com"
              autoComplete="email"
              className="w-full rounded-lg border border-white/5 bg-surface-hover px-4 py-2 text-sm outline-none focus:border-accent"
            />
            {email.trim() && !emailOk && (
              <p className="text-xs text-red-400">邮箱格式不正确</p>
            )}
          </div>
          <input
            type="text"
            value={code}
            onChange={(e) => setCode(e.target.value)}
            placeholder="PRISM-XXXX-XXXX-XXXX"
            className="w-full rounded-lg border border-white/5 bg-surface-hover px-4 py-2 font-mono text-sm outline-none focus:border-accent"
          />
          <p className="text-xs text-gray-500">
            首次激活将绑定邮箱，之后请使用同一邮箱激活或换机
          </p>
          <div className="flex justify-end gap-2">
            <button
              onClick={onClose}
              className="rounded-lg px-4 py-2 text-sm text-gray-400 hover:text-gray-200"
            >
              取消
            </button>
            <button
              onClick={submitCode}
              disabled={busy || !code.trim() || !emailOk}
              className="flex items-center gap-1 rounded-lg bg-accent px-5 py-2 text-sm transition-colors hover:bg-accent-hover disabled:opacity-50"
            >
              {busy && <Loader2 size={14} className="animate-spin" />}
              激活
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * Pro 门控：未授权时给 children 盖毛玻璃遮罩 + 激活入口；
 * 已授权（active/grace）时完全透明穿透。
 */
export default function ProGate({
  feature,
  children,
}: {
  feature: Feature;
  children: ReactNode;
}) {
  const unlocked = useProStore((s) => s.unlocked);
  const ready = useProStore((s) => s.ready);
  const [showActivate, setShowActivate] = useState(false);

  if (unlocked) return <>{children}</>;

  // 启动瞬间（授权状态尚未读出）先保持可用，避免闪烁误锁
  if (!ready) return <>{children}</>;

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
