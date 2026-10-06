import { useEffect, useState } from "react";
import { Check, Download, Lock, ShieldAlert, Sparkles } from "lucide-react";
import { listen } from "@tauri-apps/api/event";

import {
  KERNEL_DOWNLOAD_EVENT,
  ensureKernel,
  getCoreStatus,
  getKernelInfo,
  getSettings,
  saveSettings,
  setMode,
  type KernelDownloadProgress,
  type KernelInfo,
  type EntitlementState,
} from "@/api/ipc";
import type { OutboundMode, RunMode, UserSettings } from "@/stores/core";
import { useCoreStore } from "@/stores/core";
import { ActivateProModal } from "@/components/ProGate";
import { useProStore } from "@/stores/pro";
import { toast } from "@/components/ui/Toast";

const STATUS_META: Record<
  EntitlementState,
  { label: string; dot: string }
> = {
  active: { label: "已激活", dot: "bg-latency-good" },
  grace: { label: "离线宽限中", dot: "bg-yellow-400" },
  expired: { label: "已过期", dot: "bg-latency-bad" },
  revoked: { label: "已吊销", dot: "bg-latency-bad" },
  inactive: { label: "未授权", dot: "bg-gray-500" },
};

/** Unix 秒 → YYYY-MM-DD（本地时区） */
function fmtDate(secs: number | null): string {
  if (!secs) return "—";
  return new Date(secs * 1000).toLocaleDateString();
}

const MODES: { key: RunMode; title: string; desc: string }[] = [
  {
    key: "systemProxy",
    title: "系统代理",
    desc: "修改系统代理设置，仅接管支持代理的应用流量",
  },
  {
    key: "tun",
    title: "TUN 全局",
    desc: "虚拟网卡接管全部流量，需要管理员/root 权限",
  },
  {
    key: "ruleOnly",
    title: "仅规则",
    desc: "不修改系统设置，手动配置应用代理后按规则分流",
  },
];

const OUTBOUND_MODES: { key: OutboundMode; title: string; desc: string }[] = [
  {
    key: "rule",
    title: "规则",
    desc: "按订阅与自定义规则分流（默认）",
  },
  {
    key: "global",
    title: "全局",
    desc: "全部流量走 GLOBAL 组所选节点",
  },
  {
    key: "direct",
    title: "直连",
    desc: "全部流量直连，不走任何节点",
  },
];

export default function Settings() {
  const [mode, setRunMode] = useState<RunMode>("systemProxy");
  const [modeBusy, setModeBusy] = useState(false);
  const [form, setForm] = useState<UserSettings>({
    mixedPort: 2080,
    allowLan: false,
    systemProxy: true,
    mode: "systemProxy",
    autoStart: false,
    proUnlocked: false,
    licenseServerUrl: null,
    outboundMode: "rule",
    ipv6: false,
    blockQuic: false,
    closeConnectionsOnSwitch: false,
  });
  const [saved, setSaved] = useState(false);
  const [showActivate, setShowActivate] = useState(false);
  const proUnlocked = useProStore((s) => s.unlocked);
  const entitlement = useProStore((s) => s.entitlement);
  const proBusy = useProStore((s) => s.busy);
  const refreshPro = useProStore((s) => s.refresh);
  const deactivatePro = useProStore((s) => s.deactivate);
  // TUN 为 Pro 功能（全平台门控）
  const tunGated = !proUnlocked;

  // 内核信息与下载状态
  const [kernelInfo, setKernelInfo] = useState<KernelInfo | null>(null);
  const [kernelBusy, setKernelBusy] = useState(false);
  const [kernelMsg, setKernelMsg] = useState<string | null>(null);

  useEffect(() => {
    getSettings()
      .then((s) => setForm(s))
      .catch((e) => console.error(e));
    getCoreStatus()
      .then((data) => {
        setRunMode(data.mode);
        useCoreStore.getState().setMode(data.mode);
      })
      .catch(() => undefined);
    getKernelInfo()
      .then(setKernelInfo)
      .catch((e) => console.error(e));

    const un = listen<KernelDownloadProgress>(KERNEL_DOWNLOAD_EVENT, (event) => {
      const p = event.payload;
      if (p.stage === "ready" || p.stage === "error") {
        setKernelBusy(false);
        getKernelInfo().then(setKernelInfo).catch(() => undefined);
      } else {
        setKernelBusy(true);
      }
      setKernelMsg(
        p.percent > 0 ? `${p.message}（${p.percent}%）` : p.message
      );
    });
    return () => {
      un.then((fn) => fn());
    };
  }, []);

  const handleEnsureKernel = async () => {
    setKernelBusy(true);
    setKernelMsg("正在准备内核…");
    try {
      const info = await ensureKernel();
      setKernelInfo(info);
      setKernelMsg(info.exists ? "内核已就绪" : "内核未就位");
    } catch (e) {
      setKernelMsg(String(e));
    } finally {
      setKernelBusy(false);
    }
  };

  const patch = (p: Partial<UserSettings>) => {
    setForm((f) => ({ ...f, ...p }));
    setSaved(false);
  };

  const chooseMode = async (m: RunMode) => {
    if (m === "tun" && tunGated) {
      setShowActivate(true);
      return;
    }
    if (modeBusy || m === mode) return;
    setRunMode(m);
    useCoreStore.getState().setMode(m);
    setSaved(false);
    // 内核运行中切换模式会重建配置并重启内核，等待完成后重新对齐状态
    setModeBusy(true);
    try {
      await setMode(m);
      const data = await getCoreStatus();
      useCoreStore.getState().setStatus(data.status);
      useCoreStore.getState().setMode(data.mode);
    } catch (e) {
      console.error(e);
    } finally {
      setModeBusy(false);
    }
  };

  const handleSave = async () => {
    try {
      await saveSettings(form);
      setSaved(true);
    } catch (e) {
      console.error("save settings failed:", e);
    }
  };

  return (
    <div className="space-y-6 max-w-3xl">
      <h1 className="text-2xl font-bold">设置</h1>

      {/* 运行模式 */}
      <section className="bg-surface-card rounded-xl p-5">
        <h2 className="text-base font-semibold mb-4">运行模式</h2>
        <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
          {MODES.map((m) => (
            <button
              key={m.key}
              onClick={() => chooseMode(m.key)}
              disabled={modeBusy}
              className={`text-left p-4 rounded-lg border transition-all disabled:opacity-60 ${
                mode === m.key
                  ? "border-accent bg-accent/10"
                  : "border-white/5 hover:border-white/15"
              }`}
            >
              <div className="flex items-center gap-1.5 font-medium text-sm mb-1">
                {m.title}
                {m.key === "tun" && tunGated && (
                  <Lock size={13} className="text-accent" />
                )}
              </div>
              <div className="text-xs text-gray-400 leading-relaxed">
                {m.desc}
              </div>
            </button>
          ))}
        </div>
        {modeBusy && (
          <div className="mt-3 text-xs text-gray-400">
            正在切换模式（内核运行中将自动重启生效）…
          </div>
        )}

        {mode === "tun" && (
          <div className="mt-4 flex items-start gap-2 p-3 rounded-lg bg-yellow-500/10 text-yellow-400 text-xs">
            <ShieldAlert size={15} className="mt-0.5 shrink-0" />
            TUN 模式启动时若提示权限不足，请完全退出 Prism 后，右键选择
            “以管理员身份运行”（macOS/Linux 使用 sudo 启动）。
          </div>
        )}
      </section>

      {/* Pro 授权 */}
      <section className="bg-surface-card rounded-xl p-5">
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-base font-semibold flex items-center gap-2">
            <Sparkles size={16} className="text-accent" />
            Prism Pro 授权
          </h2>
          <span className="flex items-center gap-1.5 text-xs text-gray-400">
            <span
              className={`inline-block w-2 h-2 rounded-full ${STATUS_META[entitlement.status].dot}`}
            />
            {STATUS_META[entitlement.status].label}
          </span>
        </div>

        <div className="grid grid-cols-2 gap-y-2 text-xs text-gray-400 mb-4">
          <span>授权类型</span>
          <span className="text-gray-200">
            {entitlement.kind === "lifetime"
              ? "终身买断"
              : entitlement.kind === "subscription"
                ? "订阅"
                : "—"}
          </span>
          <span>到期时间</span>
          <span className="text-gray-200">{fmtDate(entitlement.expiresAt)}</span>
          <span>上次校验</span>
          <span className="text-gray-200">
            {fmtDate(entitlement.lastVerifiedAt)}
          </span>
        </div>

        <div className="flex flex-wrap gap-2">
          <button
            onClick={() => setShowActivate(true)}
            className="px-4 py-2 rounded-lg bg-accent hover:bg-accent-hover transition-colors text-sm"
          >
            {proUnlocked ? "管理授权" : "激活 Pro"}
          </button>
          <button
            onClick={refreshPro}
            disabled={proBusy}
            className="px-4 py-2 rounded-lg bg-surface-hover hover:bg-white/10 transition-colors text-sm disabled:opacity-50"
          >
            立即校验
          </button>
          {proUnlocked && (
            <button
              onClick={() => {
                if (window.confirm("确定移除本机授权？移除后 Pro 功能将锁定。")) {
                  deactivatePro()
                    .then(() => toast.info("授权已移除"))
                    .catch((e) => toast.error(String(e)));
                }
              }}
              className="px-4 py-2 rounded-lg bg-surface-hover hover:bg-latency-bad/20 transition-colors text-sm text-gray-400 hover:text-latency-bad"
            >
              移除授权
            </button>
          )}
        </div>

        {/* 授权服务地址（自建部署/联调用；留空使用内置默认） */}
        <div className="mt-4 pt-4 border-t border-white/5">
          <label className="block text-xs text-gray-500 mb-1.5">
            授权服务地址（留空使用默认；修改后点击下方「保存设置」）
          </label>
          <input
            type="text"
            value={form.licenseServerUrl ?? ""}
            onChange={(e) => patch({ licenseServerUrl: e.target.value || null })}
            placeholder="https://license.example.com"
            className="w-full px-3 py-1.5 rounded-lg bg-surface-hover border border-white/5 focus:border-accent outline-none text-sm font-mono"
          />
        </div>
      </section>

      {/* 入站设置 */}
      <section className="bg-surface-card rounded-xl p-5">
        <h2 className="text-base font-semibold mb-4">本地入站</h2>

        <div className="flex items-center justify-between py-3 border-b border-white/5">
          <div>
            <div className="text-sm">混合代理端口</div>
            <div className="text-xs text-gray-500">
              同端口同时提供 HTTP 与 SOCKS5 代理
            </div>
          </div>
          <input
            type="number"
            min={1}
            max={65535}
            value={form.mixedPort}
            onChange={(e) => patch({ mixedPort: Number(e.target.value) })}
            className="w-28 px-3 py-1.5 rounded-lg bg-surface-hover border border-white/5 focus:border-accent outline-none text-sm text-right"
          />
        </div>

        <ToggleRow
          title="允许局域网连接"
          desc="同一局域网内的其他设备可使用本机代理"
          checked={form.allowLan}
          onChange={(v) => patch({ allowLan: v })}
        />
        <ToggleRow
          title="设置系统代理"
          desc="系统代理模式启动时自动修改系统代理设置"
          checked={form.systemProxy}
          onChange={(v) => patch({ systemProxy: v })}
        />
        <ToggleRow
          title="开机自启动"
          desc="登录系统后自动启动 Prism"
          checked={form.autoStart}
          onChange={(v) => patch({ autoStart: v })}
        />
      </section>

      {/* 高级 */}
      <section className="bg-surface-card rounded-xl p-5">
        <h2 className="text-base font-semibold mb-4">高级</h2>

        <div className="py-3 border-b border-white/5">
          <div className="text-sm mb-1">出站模式</div>
          <div className="text-xs text-gray-500 mb-3">
            流量最终走向；保存后内核自动重启生效
          </div>
          <div className="grid grid-cols-3 gap-2">
            {OUTBOUND_MODES.map((m) => (
              <button
                key={m.key}
                onClick={() => patch({ outboundMode: m.key })}
                className={`text-left p-3 rounded-lg border transition-all ${
                  (form.outboundMode ?? "rule") === m.key
                    ? "border-accent bg-accent/10"
                    : "border-white/5 hover:border-white/15"
                }`}
              >
                <div className="font-medium text-sm mb-0.5">{m.title}</div>
                <div className="text-[11px] text-gray-500 leading-snug">
                  {m.desc}
                </div>
              </button>
            ))}
          </div>
        </div>

        <ToggleRow
          title="IPv6"
          desc="开启后 DNS 返回 AAAA 记录且 TUN 接管 v6 流量；关闭可避免 v6 泄漏"
          checked={form.ipv6 ?? false}
          onChange={(v) => patch({ ipv6: v })}
        />
        <ToggleRow
          title="阻止 QUIC"
          desc="拦截 UDP/443，强制浏览器回退 TCP（避免 QUIC 绕过分流）"
          checked={form.blockQuic ?? false}
          onChange={(v) => patch({ blockQuic: v })}
        />
        <ToggleRow
          title="切换策略时关闭连接"
          desc="手动切换节点后断开现有连接，新策略立即生效"
          checked={form.closeConnectionsOnSwitch ?? false}
          onChange={(v) => patch({ closeConnectionsOnSwitch: v })}
        />
      </section>

      {/* 内核管理 */}
      <section className="bg-surface-card rounded-xl p-5">
        <h2 className="text-base font-semibold mb-4">代理内核（sing-box）</h2>
        <div className="space-y-2 text-xs text-gray-400 mb-4">
          <div className="flex items-center gap-2">
            <span
              className={`inline-block w-2 h-2 rounded-full ${
                kernelInfo?.exists ? "bg-latency-good" : "bg-latency-bad"
              }`}
            />
            {kernelInfo?.exists
              ? `已安装${kernelInfo.version ? ` · v${kernelInfo.version}` : ""}`
              : "未安装（首次启动内核时将自动下载）"}
          </div>
          {kernelInfo?.path && (
            <div className="font-mono break-all text-gray-500">
              {kernelInfo.path}
            </div>
          )}
          {kernelMsg && <div className="text-gray-300">{kernelMsg}</div>}
        </div>
        <button
          onClick={handleEnsureKernel}
          disabled={kernelBusy}
          className="flex items-center gap-2 px-4 py-2 rounded-lg bg-surface-hover hover:bg-white/10 transition-colors text-sm disabled:opacity-50"
        >
          <Download size={15} className={kernelBusy ? "animate-bounce" : ""} />
          {kernelBusy ? "处理中…" : "下载 / 修复内核"}
        </button>
      </section>

      {/* 保存 */}
      <div className="flex items-center gap-4">
        <button
          onClick={handleSave}
          className="px-6 py-2 rounded-lg bg-accent hover:bg-accent-hover transition-colors text-sm font-medium"
        >
          保存设置
        </button>
        {saved && (
          <span className="flex items-center gap-1 text-latency-good text-sm">
            <Check size={15} />
            已保存（部分设置在内核重启后完全生效）
          </span>
        )}
      </div>

      <ActivateProModal
        open={showActivate}
        onClose={() => setShowActivate(false)}
      />
    </div>
  );
}

function ToggleRow({
  title,
  desc,
  checked,
  onChange,
}: {
  title: string;
  desc: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-center justify-between py-3 border-b border-white/5 last:border-0">
      <div>
        <div className="text-sm">{title}</div>
        <div className="text-xs text-gray-500">{desc}</div>
      </div>
      <button
        onClick={() => onChange(!checked)}
        className={`relative w-11 h-6 rounded-full transition-colors ${
          checked ? "bg-accent" : "bg-white/10"
        }`}
      >
        <span
          className={`absolute top-0.5 w-5 h-5 rounded-full bg-white transition-transform ${
            checked ? "translate-x-[22px]" : "translate-x-0.5"
          }`}
        />
      </button>
    </div>
  );
}
