import { useEffect, useRef, useState } from "react";
import { Pause, Play, Trash2, X, XCircle } from "lucide-react";
import { listen } from "@tauri-apps/api/event";

import {
  closeAllConnections,
  closeConnection,
} from "@/api/ipc";
import { formatBytes } from "@/utils/format";
import ProGate from "@/components/ProGate";
import { toast } from "@/components/ui/Toast";
import { Feature } from "@/pro/gating";

interface ConnRow {
  id: string;
  time: string;
  network: string;
  source: string;
  host: string;
  port: string;
  rule: string;
  outbound: string;
  active: boolean;
  upload: number;
  download: number;
}

interface Snapshot {
  downloadTotal: number;
  uploadTotal: number;
  connections: Array<{
    id: string;
    metadata: {
      network: string;
      sourceIP: string;
      sourcePort: string;
      destinationIP: string;
      destinationPort: string;
      host?: string;
    };
    upload: number;
    download: number;
    chains?: string[];
    rule?: string;
    rulePayload?: string;
  }>;
}

type FilterKey = "all" | "active" | "tcp" | "udp";

const FILTERS: { key: FilterKey; label: string }[] = [
  { key: "all", label: "全部" },
  { key: "active", label: "活跃" },
  { key: "tcp", label: "TCP" },
  { key: "udp", label: "UDP" },
];

const MAX_ROWS = 200;

export default function Connections() {
  const [rows, setRows] = useState<ConnRow[]>([]);
  const [filter, setFilter] = useState<FilterKey>("all");
  const [paused, setPaused] = useState(false);
  const [totals, setTotals] = useState({ up: 0, down: 0 });
  const pausedRef = useRef(false);

  useEffect(() => {
    pausedRef.current = paused;
  }, [paused]);

  useEffect(() => {
    const unlisten = listen<Snapshot>("connections://tick", (event) => {
      if (pausedRef.current) return;

      const snapshot = event.payload;
      setTotals({
        up: snapshot.uploadTotal ?? 0,
        down: snapshot.downloadTotal ?? 0,
      });

      const liveIds = new Set(snapshot.connections.map((c) => c.id));

      setRows((prev) => {
        const byId = new Map(prev.map((r) => [r.id, r]));

        for (const c of snapshot.connections) {
          const md = c.metadata;
          const existing = byId.get(c.id);

          if (existing) {
            byId.set(c.id, {
              ...existing,
              upload: c.upload,
              download: c.download,
              active: true,
            });
          } else {
            byId.set(c.id, {
              id: c.id,
              time: new Date().toLocaleTimeString("zh-CN", { hour12: false }),
              network: md.network ?? "tcp",
              source: `${md.sourceIP}:${md.sourcePort}`,
              host: md.host || md.destinationIP,
              port: md.destinationPort,
              rule: c.rule
                ? `${c.rule}${c.rulePayload ? ` · ${c.rulePayload}` : ""}`
                : "—",
              outbound: c.chains?.[0] ?? "DIRECT",
              active: true,
              upload: c.upload,
              download: c.download,
            });
          }
        }

        // 快照中已消失的连接标记为关闭
        for (const [id, row] of byId) {
          if (!liveIds.has(id) && row.active) {
            byId.set(id, { ...row, active: false });
          }
        }

        // 新连接在前，截断到 MAX_ROWS
        return [...byId.values()].reverse().slice(0, MAX_ROWS);
      });
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const visibleRows = rows.filter((r) => {
    if (filter === "all") return true;
    if (filter === "active") return r.active;
    return r.network === filter;
  });

  /// 关闭单条：成功后乐观移除该行；失败时下一 tick 会重新出现
  const handleClose = async (id: string) => {
    try {
      await closeConnection(id);
      setRows((prev) => prev.filter((r) => r.id !== id));
    } catch (e) {
      console.error("close connection failed:", e);
      toast.error(String(e));
    }
  };

  /// 关闭内核全部连接
  const handleCloseAll = async () => {
    try {
      await closeAllConnections();
      setRows((prev) => prev.map((r) => ({ ...r, active: false })));
      toast.success("已关闭全部连接");
    } catch (e) {
      console.error("close all connections failed:", e);
      toast.error(String(e));
    }
  };

  return (
    <ProGate feature={Feature.Connections}>
      <div className="space-y-6">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <span className="text-sm text-gray-400">
              ↑ {formatBytes(totals.up)} / ↓ {formatBytes(totals.down)}
            </span>
            <button
              onClick={() => setPaused((p) => !p)}
              className="p-2 rounded-lg bg-surface-card hover:bg-surface-hover transition-colors"
              title={paused ? "继续" : "暂停"}
            >
              {paused ? <Play size={16} /> : <Pause size={16} />}
            </button>
            <button
              onClick={handleCloseAll}
              className="p-2 rounded-lg bg-surface-card hover:bg-surface-hover transition-colors"
              title="全部关闭（断开内核当前所有连接）"
            >
              <XCircle size={16} />
            </button>
            <button
              onClick={() => setRows([])}
              className="p-2 rounded-lg bg-surface-card hover:bg-surface-hover transition-colors"
              title="清空列表"
            >
              <Trash2 size={16} />
            </button>
          </div>
        </div>

        {/* 筛选器 */}
        <div className="flex gap-2">
          {FILTERS.map(({ key, label }) => (
            <button
              key={key}
              onClick={() => setFilter(key)}
              className={`px-4 py-1.5 rounded-lg text-sm transition-colors ${
                filter === key
                  ? "bg-accent text-white"
                  : "bg-surface-card text-gray-400 hover:text-gray-200"
              }`}
            >
              {label}
            </button>
          ))}
        </div>

        {/* 连接表格 */}
        <div className="bg-surface-card rounded-xl overflow-hidden overflow-x-auto">
          <table className="w-full text-sm min-w-[800px]">
            <thead>
              <tr className="text-left text-xs text-gray-500 border-b border-white/5">
                <th className="px-4 py-3 font-medium">状态</th>
                <th className="px-4 py-3 font-medium">时间</th>
                <th className="px-4 py-3 font-medium">协议</th>
                <th className="px-4 py-3 font-medium">来源</th>
                <th className="px-4 py-3 font-medium">目标</th>
                <th className="px-4 py-3 font-medium">命中规则</th>
                <th className="px-4 py-3 font-medium">出站节点</th>
                <th className="px-4 py-3 font-medium text-right">流量 ↑/↓</th>
                <th className="px-4 py-3 font-medium text-right">操作</th>
              </tr>
            </thead>
            <tbody>
              {visibleRows.length === 0 ? (
                <tr>
                  <td colSpan={9} className="px-4 py-10 text-center text-gray-500">
                    {paused ? "已暂停接收连接事件" : "暂无连接记录（内核未启动或无流量）"}
                  </td>
                </tr>
              ) : (
                visibleRows.map((r) => (
                  <tr
                    key={r.id}
                    className="border-b border-white/5 last:border-0 hover:bg-surface-hover/50"
                  >
                    <td className="px-4 py-2.5">
                      <span
                        className={`inline-block w-2 h-2 rounded-full ${
                          r.active ? "bg-latency-good" : "bg-gray-600"
                        }`}
                      />
                    </td>
                    <td className="px-4 py-2.5 text-gray-400 font-mono text-xs">
                      {r.time}
                    </td>
                    <td className="px-4 py-2.5 uppercase text-xs">{r.network}</td>
                    <td className="px-4 py-2.5 font-mono text-xs text-gray-400">
                      {r.source}
                    </td>
                    <td className="px-4 py-2.5 font-mono text-xs">
                      {r.host}
                      <span className="text-gray-500">:{r.port}</span>
                    </td>
                    <td className="px-4 py-2.5 text-xs text-gray-300">{r.rule}</td>
                    <td className="px-4 py-2.5 text-xs text-accent">{r.outbound}</td>
                    <td className="px-4 py-2.5 text-xs text-gray-400 text-right font-mono">
                      {formatBytes(r.upload)} / {formatBytes(r.download)}
                    </td>
                    <td className="px-4 py-2.5 text-right">
                      {r.active ? (
                        <button
                          onClick={() => handleClose(r.id)}
                          className="p-1.5 rounded-md text-gray-500 hover:text-latency-bad hover:bg-surface-hover transition-colors"
                          title="关闭此连接"
                        >
                          <X size={14} />
                        </button>
                      ) : (
                        <span className="text-gray-700 text-xs">—</span>
                      )}
                    </td>
                  </tr>
                ))
              )}
            </tbody>
          </table>
        </div>
      </div>
    </ProGate>
  );
}
