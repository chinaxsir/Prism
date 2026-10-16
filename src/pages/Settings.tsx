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
  Fingerprint,
  Copy,
  Smartphone,
} from "lucide-react";

import {
  ensureKernel,
  getCoreStatus,
  getDeviceId,
  getKernelInfo,
  getSettings,
  saveSettings,
  setMode,
  setOutboundMode,
  type KernelInfo,
  type EntitlementState,
} from "@/api/ipc";
import type { OutboundMode, RunMode, UserSettings } from "@/stores/core";
import { useCoreStore } from "@/stores/core";
import { OUTBOUND_MODES, RUN_MODES } from "@/constants/modes";
import { ActivateProModal } from "@/components/ProGate";
import { useProStore } from "@/stores/pro";
import { toast } from "@/components/ui/Toast";
import { isMobile } from "@/pro/gating";
import Switch from "@/components/ui/Switch";
import clsx from "clsx";

const STATUS_META: Record<
  EntitlementState,
  { label: string; dot: string }
> = {
  active: { label: "已激活", dot: "bg-latency-good" },
  offline: { label: "离线待校验", dot: "bg-yellow-400" },
  expired: { label: "已过期", dot: "bg-latency-bad" },
  revoked: { label: "已吊销", dot: "bg-latency-bad" },
  inactive: { label: "未授权", dot: "bg-gray-500" },
};

/** 服务端方案名展示（与 /api/license/status 的 plan 字段一致） */
const PLAN_LABEL: Record<string, string> = {
  free: "Free",
  pro: "Pro",
  team: "Team",
};

function planText(plan: string | null, kind: string | null): string {
  if (plan && PLAN_LABEL[plan]) return PLAN_LABEL[plan];
  if (kind === "lifetime") return "终身买断";
  if (kind === "subscription") return "订阅";
  return "—";
}

function fmtDate(secs: number | null): string {
  if (!secs) return "—";
  return new Date(secs * 1000).toLocaleDateString();
}

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
  const [outboundBusy, setOutboundBusy] = useState(false);
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
  const [deviceId, setDeviceId] = useState<string | null>(null);

  useEffect(() => {
    getDeviceId()
      .then(setDeviceId)
      .catch((e) => console.error("get device id failed:", e));
  }, []);

  const copyDeviceId = async () => {
    if (!deviceId) return;
    try {
      await navigator.clipboard.writeText(deviceId);
      toast.success("设备 ID 已复制");
    } catch {
      toast.error("复制失败，请长按手动选择");
    }
  };

  useEffect(() => {
    getSettings()
      .then((s) => setForm(s))
      .catch((e) => console.error(e));
    getCoreStatus()
      .then((data) => {
        setRunMode(data.mode);
        useCoreStore.getState().setMode(data.mode);
        // 出站模式以内核当前状态为权威（热切换后与 settings.json 一致）
        if (data.outboundMode) {
          useCoreStore.getState().setOutboundMode(data.outboundMode);
          setForm((f) => ({ ...f, outboundMode: data.outboundMode }));
        }
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

  /// 出站模式即时生效：乐观更新 → 调用 set_outbound_mode（保存并运行中热切换）
  /// → 成功 toast；失败回滚表单与全局 store 并 toast 错误。
  const chooseOutbound = async (m: OutboundMode) => {
    if (outboundBusy || (form.outboundMode ?? "rule") === m) return;
    const prev = form.outboundMode ?? "rule";
    patch({ outboundMode: m });
    useCoreStore.getState().setOutboundMode(m);
    setOutboundBusy(true);
    try {
      await setOutboundMode(m);
      toast.success("出站模式已切换");
    } catch (e) {
      console.error("set outbound mode failed:", e);
      patch({ outboundMode: prev });
      useCoreStore.getState().setOutboundMode(prev);
      toast.error(`出站模式切换失败：${String(e)}`);
    } finally {
      setOutboundBusy(false);
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

      {/* Prism Pro 授权卡片：未授权展示设备 ID + 激活入口；已授权展示协议授权信息 */}
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
        {entitlement.status === "offline" && (
          <div className="flex items-start gap-2 px-3 py-2.5 bg-yellow-500/5 text-yellow-400 text-xs border-b border-white/5">
            <ShieldAlert size={14} className="mt-0.5 shrink-0" />
            <span>
              网络不可用，授权校验失败，Pro 功能已暂停；联网恢复后自动重新校验。
            </span>
          </div>
        )}

        {/* 设备 ID：激活时与服务端绑定，点击复制 */}
        <button
          onClick={copyDeviceId}
          className="w-full flex items-center justify-between gap-3 px-3 py-3 border-b border-white/5 text-left transition-colors hover:bg-surface-hover/40"
        >
          <span className="flex items-center gap-3 min-w-0">
            <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-white/5">
              <Fingerprint size={16} className="text-gray-400" />
            </span>
            <span className="min-w-0">
              <span className="block text-sm">设备 ID</span>
              <span className="block text-[11px] text-gray-500">
                点击复制 · 激活时绑定本机
              </span>
            </span>
          </span>
          <span className="flex min-w-0 items-center gap-1.5 text-[11px] font-mono text-gray-400">
            <span className="truncate">{deviceId ?? "生成中…"}</span>
            <Copy size={12} className="shrink-0 text-gray-500" />
          </span>
        </button>

        {proUnlocked ? (
          <>
            <Row
              icon={<Sparkles size={16} className="text-accent" />}
              iconBg="bg-accent/10"
              title="当前方案"
              value={planText(entitlement.plan, entitlement.kind)}
            />
            <Row
              icon={<Check size={16} className="text-gray-400" />}
              iconBg="bg-white/5"
              title="到期时间"
              value={
                entitlement.kind === "lifetime" && !entitlement.expiresAt
                  ? "永久有效"
                  : fmtDate(entitlement.expiresAt)
              }
            />
            {entitlement.maxSeats != null && (
              <Row
                icon={<Smartphone size={16} className="text-gray-400" />}
                iconBg="bg-white/5"
                title="设备名额"
                value={`${entitlement.usedSeats ?? "?"} / ${entitlement.maxSeats}`}
              />
            )}
            <Row
              icon={<RefreshCw size={16} className="text-gray-400" />}
              iconBg="bg-white/5"
              title="上次校验"
              value={fmtDate(entitlement.lastVerifiedAt)}
            />
            <div className="flex flex-wrap gap-2 px-3 py-3 border-t border-white/5">
              <button
                onClick={refreshPro}
                disabled={proBusy}
                className="flex items-center gap-1.5 px-3 py-2 rounded-lg bg-surface-hover hover:bg-white/10 text-xs transition-colors disabled:opacity-50"
              >
                <RefreshCw size={12} className={proBusy ? "animate-spin" : ""} />
                立即校验
              </button>
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
                移除授权
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="px-3 py-3 text-xs leading-relaxed text-gray-500 border-b border-white/5">
              输入 License Key 激活后，解锁多订阅、连接管理、规则自定义等 Pro
              功能。授权联网校验，激活即绑定本机设备。
            </div>
            <div className="flex flex-wrap gap-2 px-3 py-3">
              <button
                onClick={() => setShowActivate(true)}
                className="flex-1 min-w-[120px] flex items-center justify-center gap-1.5 px-3 py-2 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-medium transition-colors"
              >
                <Sparkles size={12} />
                激活授权
              </button>
              <button
                onClick={refreshPro}
                disabled={proBusy}
                className="flex items-center gap-1.5 px-3 py-2 rounded-lg bg-surface-hover hover:bg-white/10 text-xs transition-colors disabled:opacity-50"
              >
                <RefreshCw size={12} className={proBusy ? "animate-spin" : ""} />
                立即校验
              </button>
            </div>
          </>
        )}
      </SettingsGroup>

      {/* 接入模式：iOS/Android 系统强制 TUN 接管，无切换意义，移动端不渲染 */}
      {!mobile && (
        <SettingsGroup
          title="接入模式"
          icon={<Wifi size={14} />}
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
          {mode === "tun" && (
            <div className="px-3 py-2.5 flex items-start gap-2 bg-yellow-500/5 text-yellow-400 text-xs border-t border-white/5">
              <ShieldAlert size={14} className="mt-0.5 shrink-0" />
              TUN 模式启动时若提示权限不足，请完全退出 Prism 后，右键选择
              “以管理员身份运行”（macOS/Linux 使用 sudo 启动）。
            </div>
          )}
        </SettingsGroup>
      )}

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
        desc={
          mobile
            ? "出站模式决定流量最终走向"
            : "出站模式决定流量最终走向，与上方接入模式互不影响"
        }
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
                  onClick={() => chooseOutbound(m.key)}
                  disabled={outboundBusy}
                  className={clsx(
                    "flex flex-col items-center gap-1 p-2.5 rounded-lg border transition-all disabled:opacity-60",
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
      <Switch checked={checked} onChange={onChange} />
    </div>
  );
}
