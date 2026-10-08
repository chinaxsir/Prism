import { useEffect, useState } from "react";
import {
  Check,
  Lock,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  ChevronRight,
  Wifi,
  Network,
  Sliders,
  Cpu,
  Power,
  Globe,
  Blocks,
  RefreshCw,
} from "lucide-react";

import {
  ensureKernel,
  getCoreStatus,
  getKernelInfo,
  getSettings,
  saveSettings,
  setMode,
  type KernelInfo,
  type EntitlementState,
} from "@/api/ipc";
import type { RunMode, UserSettings } from "@/stores/core";
import { useCoreStore } from "@/stores/core";
import { OUTBOUND_MODES, RUN_MODES } from "@/constants/modes";
import { ActivateProModal } from "@/components/ProGate";
import { useProStore } from "@/stores/pro";
import { toast } from "@/components/ui/Toast";
import clsx from "clsx";

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

function fmtDate(secs: number | null): string {
  if (!secs) return "—";
  return new Date(secs * 1000).toLocaleDateString();
}

const isMobile = () => {
  try {
    const ua = navigator.userAgent;
    return ua.includes("Android") || ua.includes("iPhone") || ua.includes("iPad");
  } catch {
    return false;
  }
};

/// 接入模式图标
const RUN_MODE_ICON: Record<RunMode, typeof Wifi> = {
  systemProxy: Wifi,
  tun: Network,
};

/// 出站模式图标
const OUTBOUND_ICON: Record<string, typeof Globe> = {
  rule: Sliders,
  global: Globe,
  direct: Power,
};

export default function Settings() {
  const [mode, setRunMode] = useState<RunMode>("systemProxy");
  const [modeBusy, setModeBusy] = useState(false);
  const [form, setForm] = useState<UserSettings>({
    mixedPort: 2080,
    allowLan: false,
    systemProxy: true,
    mode: "systemProxy",
    autoStart: false,
    outboundMode: "rule",
    ipv6: false,
    blockQuic: false,
    closeConnectionsOnSwitch: false,
  });
  const [saved, setSaved] = useState(false);
  const mobile = isMobile();
  const [showActivate, setShowActivate] = useState(false);
  const proUnlocked = useProStore((s) => s.unlocked);
  const entitlement = useProStore((s) => s.entitlement);
  const proBusy = useProStore((s) => s.busy);
  const refreshPro = useProStore((s) => s.refresh);
  const deactivatePro = useProStore((s) => s.deactivate);
  const tunGated = !proUnlocked;

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
  }, []);

  const handleEnsureKernel = async () => {
    setKernelBusy(true);
    setKernelMsg("正在校验内核…");
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
    setForm((f) => ({ ...f, mode: m }));
    useCoreStore.getState().setMode(m);
    setSaved(false);
    setModeBusy(true);
    try {
      await setMode(m);
      const data = await getCoreStatus();
      useCoreStore.getState().setStatus(data.status);
      useCoreStore.getState().setMode(data.mode);
      toast.success(
        data.status === "running"
          ? "模式已切换，内核已重启生效"
          : "模式已切换，下次启动内核时生效"
      );
    } catch (e) {
      console.error(e);
      toast.error(String(e));
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

  const runModes = MODES.filter((m) => (mobile ? m.key === "tun" : true));

  return (
    <div className="space-y-5 max-w-3xl">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold">设置</h1>
        <button
          onClick={handleSave}
          className={clsx(
            "px-4 py-1.5 rounded-lg text-sm font-medium transition-colors",
            saved
              ? "bg-latency-good/20 text-latency-good"
              : "bg-accent hover:bg-accent-hover text-white"
          )}
        >
          {saved ? (
            <span className="flex items-center gap-1">
              <Check size={14} />
              已保存
            </span>
          ) : (
            "保存"
          )}
        </button>
      </div>

      {/* Prism Pro 授权卡片 */}
      <SettingsGroup
        title="Prism Pro"
        icon={<Sparkles size={14} className="text-accent" />}
        headerRight={
          <span className="flex items-center gap-1.5 text-xs text-gray-400">
            <span
              className={`inline-block w-1.5 h-1.5 rounded-full ${STATUS_META[entitlement.status].dot}`}
            />
            {STATUS_META[entitlement.status].label}
          </span>
        }
      >
        <Row
          icon={<Sparkles size={16} className="text-accent" />}
          iconBg="bg-accent/10"
          title="授权类型"
          value={
            entitlement.kind === "lifetime"
              ? "终身买断"
              : entitlement.kind === "subscription"
                ? "订阅"
                : "—"
          }
        />
        <Row
          icon={<Check size={16} className="text-gray-400" />}
          iconBg="bg-white/5"
          title="到期时间"
          value={fmtDate(entitlement.expiresAt)}
        />
        <Row
          icon={<RefreshCw size={16} className="text-gray-400" />}
          iconBg="bg-white/5"
          title="上次校验"
          value={fmtDate(entitlement.lastVerifiedAt)}
        />
        <div className="flex flex-wrap gap-2 px-3 py-3 border-t border-white/5">
          <button
            onClick={() => setShowActivate(true)}
            className="flex-1 min-w-[100px] px-3 py-2 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-medium transition-colors"
          >
            {proUnlocked ? "管理授权" : "激活 Pro"}
          </button>
          <button
            onClick={refreshPro}
            disabled={proBusy}
            className="px-3 py-2 rounded-lg bg-surface-hover hover:bg-white/10 text-xs transition-colors disabled:opacity-50"
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
              className="px-3 py-2 rounded-lg bg-surface-hover hover:bg-latency-bad/20 text-xs text-gray-400 hover:text-latency-bad transition-colors"
            >
              移除
            </button>
          )}
        </div>
      </SettingsGroup>

      {/* 接入模式 */}
      <SettingsGroup
        title="接入模式"
        desc="决定流量如何进入 Prism（与下方出站模式互不影响）"
      >
        {runModes.map((m) => {
          const Icon = RUN_MODE_ICON[m.key] ?? Wifi;
          const active = mode === m.key;
          return (
            <button
              key={m.key}
              onClick={() => chooseMode(m.key)}
              disabled={modeBusy}
              className="w-full text-left disabled:opacity-60"
            >
              <Row
                icon={
                  <Icon size={16} className={active ? "text-accent" : "text-gray-400"} />
                }
                iconBg={active ? "bg-accent/10" : "bg-white/5"}
                title={
                  <span className="flex items-center gap-1.5">
                    {m.title}
                    {m.key === "tun" && tunGated && (
                      <Lock size={12} className="text-accent" />
                    )}
                  </span>
                }
                value={active ? "已选" : ""}
                chevron={!active}
                valueClass={active ? "text-accent" : ""}
              />
            </button>
          );
        })}
        {modeBusy && (
          <div className="px-3 py-2 text-xs text-gray-400 border-t border-white/5">
            正在切换模式（内核运行中将自动重启生效）…
          </div>
        )}
        {!mobile && mode === "tun" && (
          <div className="px-3 py-2.5 flex items-start gap-2 bg-yellow-500/5 text-yellow-400 text-xs border-t border-white/5">
            <ShieldAlert size={14} className="mt-0.5 shrink-0" />
            TUN 模式启动时若提示权限不足，请完全退出 Prism 后，右键选择
            “以管理员身份运行”（macOS/Linux 使用 sudo 启动）。
          </div>
        )}
      </SettingsGroup>

      {/* 本地入站 */}
      <SettingsGroup title="本地入站" icon={<Network size={14} />}>
        <div className="flex items-center justify-between px-3 py-3 border-b border-white/5">
          <div className="flex items-center gap-3">
            <RowIcon><Network size={16} className="text-gray-400" /></RowIcon>
            <div>
              <div className="text-sm">混合代理端口</div>
              <div className="text-[11px] text-gray-500">同端口提供 HTTP 与 SOCKS5</div>
            </div>
          </div>
          <input
            type="number"
            min={1}
            max={65535}
            value={form.mixedPort}
            onChange={(e) => patch({ mixedPort: Number(e.target.value) })}
            className="w-24 px-3 py-1.5 rounded-lg bg-surface-hover border border-white/5 focus:border-accent outline-none text-sm text-right"
          />
        </div>
        <ToggleRow
          icon={<Globe size={16} className="text-gray-400" />}
          title="允许局域网连接"
          desc="同局域网其他设备可使用本机代理"
          checked={form.allowLan}
          onChange={(v) => patch({ allowLan: v })}
        />
        {!mobile && (
          <ToggleRow
            icon={<Wifi size={16} className="text-gray-400" />}
            title="设置系统代理"
            desc="系统代理模式启动时自动修改系统代理设置"
            checked={form.systemProxy}
            onChange={(v) => patch({ systemProxy: v })}
          />
        )}
        {!mobile && (
          <ToggleRow
            icon={<Power size={16} className="text-gray-400" />}
            title="开机自启动"
            desc="登录系统后自动启动 Prism"
            checked={form.autoStart}
            onChange={(v) => patch({ autoStart: v })}
          />
        )}
      </SettingsGroup>

      {/* 高级 */}
      <SettingsGroup
        title="高级"
        icon={<Sliders size={14} />}
        desc="出站模式决定流量最终走向，与接入模式互不影响"
      >
        <div className="px-3 py-3 border-b border-white/5">
          <div className="flex items-center gap-3 mb-3">
            <RowIcon><Blocks size={16} className="text-gray-400" /></RowIcon>
            <div className="text-sm">出站模式</div>
          </div>
          <div className="grid grid-cols-3 gap-2">
            {OUTBOUND_MODES_LOCAL.map((m) => {
              const Icon = OUTBOUND_ICON[m.key] ?? Globe;
              const active = (form.outboundMode ?? "rule") === m.key;
              return (
                <button
                  key={m.key}
                  onClick={() => patch({ outboundMode: m.key })}
                  className={clsx(
                    "flex flex-col items-center gap-1 p-2.5 rounded-lg border transition-all",
                    active
                      ? "border-accent bg-accent/10"
                      : "border-white/5 hover:border-white/15"
                  )}
                >
                  <Icon size={16} className={active ? "text-accent" : "text-gray-400"} />
                  <div className="text-xs font-medium">{m.title}</div>
                </button>
              );
            })}
          </div>
        </div>
        <ToggleRow
          icon={<Globe size={16} className="text-gray-400" />}
          title="IPv6"
          desc="开启后 DNS 返回 AAAA 并 TUN 接管 v6 流量"
          checked={form.ipv6 ?? false}
          onChange={(v) => patch({ ipv6: v })}
        />
        <ToggleRow
          icon={<Blocks size={16} className="text-gray-400" />}
          title="阻止 QUIC"
          desc="拦截 UDP/443 强制 TCP 回退"
          checked={form.blockQuic ?? false}
          onChange={(v) => patch({ blockQuic: v })}
        />
        <ToggleRow
          icon={<RefreshCw size={16} className="text-gray-400" />}
          title="切换策略时关闭连接"
          desc="切换节点后断开现有连接，新策略立即生效"
          checked={form.closeConnectionsOnSwitch ?? false}
          onChange={(v) => patch({ closeConnectionsOnSwitch: v })}
        />
      </SettingsGroup>

      {/* 代理内核 */}
      <SettingsGroup title="代理内核" icon={<Cpu size={14} />}>
        <Row
          icon={
            <ShieldCheck
              size={16}
              className={kernelInfo?.exists ? "text-latency-good" : "text-latency-bad"}
            />
          }
          iconBg={kernelInfo?.exists ? "bg-latency-good/10" : "bg-latency-bad/10"}
          title={
            kernelInfo?.exists
              ? `已安装${kernelInfo.version ? ` · v${kernelInfo.version}` : ""}`
              : "内核文件缺失"
          }
          value={kernelInfo?.exists ? "" : "请重新安装"}
          valueClass="text-latency-bad"
        />
        {kernelInfo?.path && (
          <div className="px-3 py-2 text-[11px] font-mono break-all text-gray-500 border-b border-white/5">
            {kernelInfo.path}
          </div>
        )}
        {kernelMsg && (
          <div className="px-3 py-2 text-xs text-gray-300 border-b border-white/5">
            {kernelMsg}
          </div>
        )}
        <div className="px-3 py-3">
          <button
            onClick={handleEnsureKernel}
            disabled={kernelBusy}
            className="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-surface-hover hover:bg-white/10 text-xs transition-colors disabled:opacity-50"
          >
            <ShieldCheck size={13} className={kernelBusy ? "animate-pulse" : ""} />
            {kernelBusy ? "处理中…" : "校验内核状态"}
          </button>
        </div>
      </SettingsGroup>

      <ActivateProModal
        open={showActivate}
        onClose={() => setShowActivate(false)}
      />
    </div>
  );
}

const MODES = RUN_MODES;
const OUTBOUND_MODES_LOCAL = OUTBOUND_MODES;

// ---- 复用组件 ----

function SettingsGroup({
  title,
  desc,
  icon,
  headerRight,
  children,
}: {
  title: string;
  desc?: string;
  icon?: React.ReactNode;
  headerRight?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section>
      <div className="flex items-center justify-between px-3 mb-1.5">
        <div className="flex items-center gap-1.5 text-xs font-medium text-gray-400 uppercase tracking-wide">
          {icon}
          <span>{title}</span>
        </div>
        {headerRight}
      </div>
      {desc && (
        <p className="px-3 mb-1.5 text-[11px] text-gray-500 leading-relaxed">
          {desc}
        </p>
      )}
      <div className="rounded-xl bg-surface-card border border-white/5 overflow-hidden">
        {children}
      </div>
    </section>
  );
}

function RowIcon({ children }: { children: React.ReactNode }) {
  return (
    <span className="flex h-7 w-7 items-center justify-center rounded-md bg-white/5">
      {children}
    </span>
  );
}

function Row({
  icon,
  iconBg,
  title,
  value,
  valueClass,
  chevron,
}: {
  icon: React.ReactNode;
  iconBg?: string;
  title: React.ReactNode;
  value?: React.ReactNode;
  valueClass?: string;
  chevron?: boolean;
}) {
  return (
    <div className="flex items-center justify-between px-3 py-3 border-b border-white/5 last:border-0">
      <div className="flex items-center gap-3 flex-1 min-w-0">
        <span
          className={clsx(
            "flex h-7 w-7 items-center justify-center rounded-md",
            iconBg ?? "bg-white/5"
          )}
        >
          {icon}
        </span>
        <div className="text-sm truncate">{title}</div>
      </div>
      {value !== undefined && value !== "" && (
        <span className={clsx("text-xs text-gray-400", valueClass)}>{value}</span>
      )}
      {chevron && <ChevronRight size={14} className="text-gray-600 ml-2" />}
    </div>
  );
}

function ToggleRow({
  icon,
  title,
  desc,
  checked,
  onChange,
}: {
  icon?: React.ReactNode;
  title: string;
  desc: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-center justify-between px-3 py-3 border-b border-white/5 last:border-0">
      <div className="flex items-center gap-3 flex-1 min-w-0">
        {icon && (
          <span className="flex h-7 w-7 items-center justify-center rounded-md bg-white/5">
            {icon}
          </span>
        )}
        <div className="min-w-0">
          <div className="text-sm truncate">{title}</div>
          <div className="text-[11px] text-gray-500 truncate">{desc}</div>
        </div>
      </div>
      <button
        onClick={() => onChange(!checked)}
        className={clsx(
          "relative w-11 h-6 rounded-full transition-colors shrink-0",
          checked ? "bg-accent" : "bg-white/10"
        )}
      >
        <span
          className={clsx(
            "absolute top-0.5 w-5 h-5 rounded-full bg-white transition-transform",
            checked ? "translate-x-[22px]" : "translate-x-0.5"
          )}
        />
      </button>
    </div>
  );
}
