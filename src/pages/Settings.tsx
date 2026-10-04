import { useEffect, useState } from "react";
import { Check, Download, ShieldAlert } from "lucide-react";
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
} from "@/api/ipc";
import type { RunMode, UserSettings } from "@/stores/core";

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

export default function Settings() {
  const [mode, setRunMode] = useState<RunMode>("systemProxy");
  const [form, setForm] = useState<UserSettings>({
    mixedPort: 2080,
    allowLan: false,
    systemProxy: true,
    autoStart: false,
  });
  const [saved, setSaved] = useState(false);

  // 内核信息与下载状态
  const [kernelInfo, setKernelInfo] = useState<KernelInfo | null>(null);
  const [kernelBusy, setKernelBusy] = useState(false);
  const [kernelMsg, setKernelMsg] = useState<string | null>(null);

  useEffect(() => {
    getSettings()
      .then((s) => setForm(s))
      .catch((e) => console.error(e));
    getCoreStatus()
      .then((dto) => {
        const data = dto as { mode?: RunMode };
        if (data.mode) setRunMode(data.mode);
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

  const chooseMode = (m: RunMode) => {
    setRunMode(m);
    setMode(m).catch((e) => console.error(e));
    setSaved(false);
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
        <div className="grid grid-cols-3 gap-3">
          {MODES.map((m) => (
            <button
              key={m.key}
              onClick={() => chooseMode(m.key)}
              className={`text-left p-4 rounded-lg border transition-all ${
                mode === m.key
                  ? "border-accent bg-accent/10"
                  : "border-white/5 hover:border-white/15"
              }`}
            >
              <div className="font-medium text-sm mb-1">{m.title}</div>
              <div className="text-xs text-gray-400 leading-relaxed">
                {m.desc}
              </div>
            </button>
          ))}
        </div>

        {mode === "tun" && (
          <div className="mt-4 flex items-start gap-2 p-3 rounded-lg bg-yellow-500/10 text-yellow-400 text-xs">
            <ShieldAlert size={15} className="mt-0.5 shrink-0" />
            TUN 模式启动时若提示权限不足，请完全退出 Prism 后，右键选择
            “以管理员身份运行”（macOS/Linux 使用 sudo 启动）。
          </div>
        )}
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
