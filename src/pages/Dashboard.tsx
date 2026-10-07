import { useEffect, useState, type ReactNode } from "react";
import {
  Activity,
  Cpu,
  Gauge,
  Power,
  Timer,
  BarChart3,
} from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import {
  CartesianGrid,
  Legend,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

import { getCoreStatus, startCore, stopCore } from "@/api/ipc";
import { useCoreStore, type RunMode } from "@/stores/core";
import { formatBytes, formatSpeed, formatUptime } from "@/utils/format";

const MODE_LABEL: Record<RunMode, string> = {
  systemProxy: "系统代理",
  tun: "TUN 全局",
};

// 平台检测：Tauri 内部 API 或 fallback
const isMobile = () => {
  try {
    return navigator.userAgent.includes("Android") || navigator.userAgent.includes("iPhone") || navigator.userAgent.includes("iPad");
  } catch {
    return false;
  }
};

export default function Dashboard() {
  const status = useCoreStore((s) => s.status);
  const mode = useCoreStore((s) => s.mode);
  const upSpeed = useCoreStore((s) => s.upSpeed);
  const downSpeed = useCoreStore((s) => s.downSpeed);
  const memoryUsage = useCoreStore((s) => s.memoryUsage);
  const uptimeSecs = useCoreStore((s) => s.uptimeSecs);
  const traffic = useCoreStore((s) => s.traffic);
  const sessionTraffic = useCoreStore((s) => s.sessionTraffic);
  const setStatus = useCoreStore((s) => s.setStatus);
  const setMode = useCoreStore((s) => s.setMode);
  const setTrafficRate = useCoreStore((s) => s.setTrafficRate);
  const setMemoryUsage = useCoreStore((s) => s.setMemoryUsage);
  const setUptime = useCoreStore((s) => s.setUptime);
  const pushTraffic = useCoreStore((s) => s.pushTraffic);

  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const mobile = isMobile();

  const running = status === "running";

  // 订阅内核实时事件
  useEffect(() => {
    const unlistenTraffic = listen<{ up: number; down: number }>(
      "traffic://tick",
      (event) => {
        const { up, down } = event.payload;
        setTrafficRate(up, down);
        pushTraffic(up, down);
      }
    );

    const unlistenMemory = listen<{ inuse: number }>(
      "memory://tick",
      (event) => {
        setMemoryUsage(event.payload.inuse ?? 0);
      }
    );

    return () => {
      unlistenTraffic.then((fn) => fn());
      unlistenMemory.then((fn) => fn());
    };
  }, [setTrafficRate, setMemoryUsage, pushTraffic]);

  // 本地运行时长计时
  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => {
      setUptime(useCoreStore.getState().uptimeSecs + 1);
    }, 1000);
    return () => clearInterval(timer);
  }, [running, setUptime]);

  // 启动时与后端对齐一次状态（含运行模式：权威来源是后端持久化的 settings.json）
  useEffect(() => {
    getCoreStatus()
      .then((data) => {
        setStatus(data.status);
        setMode(data.mode);
        setUptime(data.uptimeSecs);
      })
      .catch(() => undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const toggle = async () => {
    setErrorMsg(null);
    try {
      if (running) {
        await stopCore();
        setStatus("stopped");
        setUptime(0);
      } else {
        setStatus("starting");
        await startCore();
        setStatus("running");
      }
    } catch (e) {
      console.error("toggle core failed:", e);
      setStatus("error");
      setErrorMsg(String(e));
    }
  };

  return (
    <div className="space-y-6">
      <h1 className="text-2xl font-bold">仪表盘</h1>

      {/* 状态卡片 */}
      <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-4">
        <button
          onClick={toggle}
          className={`rounded-xl p-5 flex flex-col items-start justify-between h-28 transition-all duration-200 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-ring ${running ? "bg-latency-bad/20 hover:bg-latency-bad/30" : "bg-accent/20 hover:bg-accent/30"}`}
        >
          <Power size={24} className={running ? "text-latency-bad" : "text-accent"} />
          <span className="text-xl font-bold">
            {running ? "停止内核" : "启动内核"}
          </span>
        </button>

        <StatCard
          icon={<Gauge size={20} className="text-accent" />}
          label="运行模式"
          value={MODE_LABEL[mode]}
        />
        <StatCard
          icon={<Timer size={20} className="text-accent" />}
          label="运行时长"
          value={running ? formatUptime(uptimeSecs) : "—"}
        />
        {mobile ? (
          <StatCard
            icon={<BarChart3 size={20} className="text-accent" />}
            label="今日流量"
            value={running ? formatBytes(sessionTraffic) : "—"}
          />
        ) : (
          <StatCard
            icon={<Cpu size={20} className="text-accent" />}
            label="内存占用"
            value={running && memoryUsage > 0 ? formatBytes(memoryUsage) : "—"}
          />
        )}
      </div>

      {/* 启动/停止错误提示 */}
      {errorMsg && (
        <div className="rounded-xl p-4 text-sm shadow-md bg-latency-bad/20 text-red-300 border border-red-500/30">
          <div className="font-semibold mb-1">操作失败</div>
          <div className="text-xs break-all text-red-200">{errorMsg}</div>
        </div>
      )}

      {/* 实时速率 */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <div className="bg-surface-card rounded-xl p-5">
          <div className="flex items-center gap-2 text-sm text-gray-400">
            <Activity size={16} className="text-latency-good" />
            下载速率
          </div>
          <div className="text-2xl font-bold text-latency-good mt-2">
            {running ? formatSpeed(downSpeed) : "—"}
          </div>
        </div>
        <div className="bg-surface-card rounded-xl p-5">
          <div className="flex items-center gap-2 text-sm text-gray-400">
            <Activity size={16} className="text-accent" />
            上传速率
          </div>
          <div className="text-2xl font-bold text-accent mt-2">
            {running ? formatSpeed(upSpeed) : "—"}
          </div>
        </div>
      </div>

      {/* 流量曲线图 */}
      <div className="bg-surface-card rounded-xl p-5 shadow-md">
        <h2 className="text-lg font-semibold mb-4 text-gray-100">实时流量（最近 60 秒）</h2>
        <ResponsiveContainer width="100%" height={280}>
          <LineChart data={traffic} margin={{ top: 4, right: 12, bottom: 0, left: 0 }}>
            <CartesianGrid strokeDasharray="3 3" stroke="rgba(255,255,255,0.1)" />
            <XAxis
              dataKey="time"
              stroke="#9ca3af"
              fontSize={11}
              tickLine={false}
              minTickGap={48}
            />
            <YAxis
              stroke="#9ca3af"
              fontSize={11}
              tickLine={false}
              width={72}
              tickFormatter={(v) => formatBytes(Number(v))}
            />
            <Tooltip
              contentStyle={{
                backgroundColor: "#1E222D",
                border: "1px solid rgba(255,255,255,0.15)",
                borderRadius: 8,
                fontSize: 12,
                padding: '8px 12px',
              }}
              labelStyle={{ color: '#E5E7EB' }}
              itemStyle={{ color: '#E5E7EB' }}
              formatter={(value, name) => [
                formatSpeed(Number(value)),
                name === "down" ? "下载" : "上传",
              ]}
            />
            <Legend
              formatter={(v) => (v === "down" ? "下载" : "上传")}
              wrapperStyle={{ paddingTop: '16px' }}
            />
            <Line
              type="monotone"
              dataKey="down"
              stroke="#22c55e"
              strokeWidth={2}
              dot={false}
              isAnimationActive={false}
            />
            <Line
              type="monotone"
              dataKey="up"
              stroke="#6366f1"
              strokeWidth={2}
              dot={false}
              isAnimationActive={false}
            />
          </LineChart>
        </ResponsiveContainer>
      </div>
    </div>
  );
}

function StatCard({
  icon,
  label,
  value,
}: {
  icon: ReactNode;
  label: string;
  value: string;
}) {
  return (
    <div className="bg-surface-card rounded-xl p-5 h-28 flex flex-col justify-between shadow-md">
      <div className="flex items-center gap-2 text-sm text-gray-400">
        {icon}
        <span className="font-medium">{label}</span>
      </div>
      <div className="text-2xl font-bold text-gray-100">{value}</div>
    </div>
  );
}
