import { useEffect, useState, type ReactNode } from "react";
import { Check, Loader2, Lock, Sparkles } from "lucide-react";

import { FEATURE_LABEL, isMobile, type Feature } from "@/pro/gating";
import {
  STORE_PRODUCTS,
  buyProduct,
  fetchStoreProducts,
  restoreAll,
} from "@/pro/iap";
import type { Entitlement } from "@/api/ipc";
import { useProStore } from "@/stores/pro";
import type { Product } from "@choochmeque/tauri-plugin-iap-api";
import { toast } from "@/components/ui/Toast";

/** 服务端 429 时从错误信息中解析 retry_after 秒数（容忍 JSON / 纯文本格式） */
function parseRetryAfter(message: string): number {
  if (!message) return 0;
  const explicit = message.match(/retry[_\s-]*after[^0-9]{0,12}(\d{1,5})/i);
  if (explicit) return parseInt(explicit[1], 10);
  const plain = message.match(/(\d{1,5})\s*(?:秒|seconds?|s)\b/i);
  if (plain) return parseInt(plain[1], 10);
  return 0;
}

const PLAN_LABEL: Record<string, string> = {
  free: "Free",
  pro: "Pro",
  team: "Team",
};

/** 展示当前方案：优先服务端 plan 字段（free/pro/team），回落到授权类型 */
function planLabel(e: Entitlement): string {
  if (e.plan && PLAN_LABEL[e.plan]) return PLAN_LABEL[e.plan];
  if (e.kind === "lifetime") return "终身买断";
  if (e.kind === "subscription") return "订阅";
  return "—";
}

function expiryText(e: Entitlement): string {
  if (e.kind === "lifetime" && !e.expiresAt) return "永久有效";
  if (!e.expiresAt) return "—";
  return new Date(e.expiresAt * 1000).toLocaleDateString();
}

/// 激活 Pro 弹窗：License Key（全平台）+ 商店内购（仅移动端）
export function ActivateProModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const activate = useProStore((s) => s.activate);
  const error = useProStore((s) => s.error);
  const [licenseKey, setLicenseKey] = useState("");
  const [email, setEmail] = useState("");
  const [busy, setBusy] = useState(false);
  const [retryAfter, setRetryAfter] = useState(0);
  const [activated, setActivated] = useState<Entitlement | null>(null);
  const [products, setProducts] = useState<Product[]>([]);
  const mobile = isMobile();

  // 429 倒计时：每秒递减，归零后恢复按钮
  useEffect(() => {
    if (retryAfter <= 0) return;
    const timer = setTimeout(() => setRetryAfter((s) => Math.max(0, s - 1)), 1000);
    return () => clearTimeout(timer);
  }, [retryAfter]);

  useEffect(() => {
    if (!open || !mobile) return;
    fetchStoreProducts()
      .then(setProducts)
      .catch((e) => console.warn("fetch products failed:", e));
  }, [open, mobile]);

  if (!open) return null;

  const emailOk = /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email.trim());
  const counting = retryAfter > 0;

  // License Key 激活（邮箱可选，激活时提交）
  const submitLicense = async () => {
    if (busy || counting || !licenseKey.trim()) return;
    if (email.trim() && !emailOk) return;
    setBusy(true);
    const ok = await activate(licenseKey.trim(), email.trim() || undefined);
    setBusy(false);
    if (ok) {
      toast.success("Pro 已激活，高级功能全部解锁");
      setActivated(useProStore.getState().entitlement);
      setLicenseKey("");
      setEmail("");
    } else {
      const wait = parseRetryAfter(error ?? "");
      if (wait > 0) setRetryAfter(wait);
      toast.error(
        wait > 0
          ? `激活尝试过于频繁，请 ${wait} 秒后重试`
          : (error ?? "激活失败，请检查 License Key 与邮箱")
      );
    }
  };

  // 关闭并复位成功态（倒计时保留，避免限流期内立即重试）
  const dismiss = () => {
    setActivated(null);
    onClose();
  };

  // 移动端商店购买
  const buy = async (productId: string, type: "inapp" | "subs") => {
    if (busy) return;
    setBusy(true);
    try {
      await buyProduct(productId, type);
      toast.success("购买成功，Pro 已激活");
      dismiss();
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
        dismiss();
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
      onClick={dismiss}
    >
      <div
        className="max-h-[90vh] w-full max-w-sm space-y-5 overflow-y-auto rounded-xl bg-surface-card p-6"
        onClick={(e) => e.stopPropagation()}
      >
        {activated ? (
          <>
            <div className="flex items-center gap-2.5">
              <span className="flex h-8 w-8 items-center justify-center rounded-full bg-latency-good/15">
                <Check size={16} className="text-latency-good" />
              </span>
              <h3 className="text-base font-semibold">激活成功</h3>
            </div>
            <div className="space-y-2.5 rounded-lg border border-white/5 bg-surface-hover px-4 py-3.5 text-sm">
              <div className="flex items-center justify-between">
                <span className="text-gray-400">当前方案</span>
                <span className="font-medium text-gray-100">
                  {planLabel(activated)}
                </span>
              </div>
              <div className="flex items-center justify-between">
                <span className="text-gray-400">到期时间</span>
                <span className="font-medium tabular-nums text-gray-100">
                  {expiryText(activated)}
                </span>
              </div>
            </div>
            <button
              onClick={dismiss}
              className="w-full rounded-lg bg-accent px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-accent-hover"
            >
              完成
            </button>
          </>
        ) : (
          <>
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
                或使用 License Key
                <div className="h-px flex-1 bg-white/5" />
              </div>
            )}

            {/* License Key + 邮箱（可选） */}
            <div className="space-y-3">
              <div className="space-y-1.5">
                <label className="text-xs text-gray-400">License Key</label>
                <input
                  type="text"
                  value={licenseKey}
                  onChange={(e) => setLicenseKey(e.target.value)}
                  placeholder="ASKC-XXXX-XXXX-XXXX-XXXX"
                  autoComplete="off"
                  spellCheck={false}
                  className="w-full rounded-lg border border-white/5 bg-surface-hover px-4 py-2 font-mono text-sm outline-none focus:border-accent"
                />
              </div>
              <div className="space-y-1.5">
                <label className="text-xs text-gray-400">邮箱（可选）</label>
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
              <p className="text-xs text-gray-500">
                邮箱为可选，用于授权校验；若该 License Key 需邮箱验证，请填写购买时预留的邮箱
              </p>
              <div className="flex justify-end gap-2">
                <button
                  onClick={dismiss}
                  className="rounded-lg px-4 py-2 text-sm text-gray-400 hover:text-gray-200"
                >
                  取消
                </button>
                <button
                  onClick={submitLicense}
                  disabled={
                    busy ||
                    counting ||
                    !licenseKey.trim() ||
                    (email.trim() !== "" && !emailOk)
                  }
                  className="flex items-center gap-1 rounded-lg bg-accent px-5 py-2 text-sm transition-colors hover:bg-accent-hover disabled:opacity-50"
                >
                  {busy && <Loader2 size={14} className="animate-spin" />}
                  {counting ? `请 ${retryAfter} 秒后重试` : "激活"}
                </button>
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

/**
 * Pro 门控：未授权时给 children 盖毛玻璃遮罩 + 激活入口；
 * 已授权（active，服务端联网校验有效）时完全透明穿透。
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
