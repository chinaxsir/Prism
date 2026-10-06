import { useEffect, useRef, useState } from "react";
import { Globe2, ListFilter, PieChart } from "lucide-react";
import { listen } from "@tauri-apps/api/event";

import { getRules } from "@/api/ipc";
import { useCoreStore } from "@/stores/core";
import { formatBytes } from "@/utils/format";

interface ConnItem {
  id: string;
  metadata: { host?: string; destinationIP?: string };
  upload: number;
  download: number;
  chains?: string[];
  rule?: string;
  rulePayload?: string;
}

interface Snapshot {
  connections: ConnItem[];
}

interface Agg {
  count: number;
  down: number;
  up: number;
}

interface RuleItem {
  type: string;
  payload: string;
  proxy: string;
}

const TOP_N = 8;

/// 分析页：主机名/策略/规则聚合统计。
/// 聚合数据在本页挂载期间累积（connections://tick 为增量快照）。
export default function Analytics() {
  const status = useCoreStore((s) => s.status);
  const running = status === "running";

  // 连接聚合：主机名 / 出站策略 / 命中规则（上次快照差量累加流量）
  const aggRef = useRef({
    hosts: new Map<string, Agg>(),
    outbounds: new Map<string, Agg>(),
    rules: new Map<string, Agg>(),
    lastSeen: new Map<string, { up: number; down: number }>(),
  });
  const [hostTop, setHostTop] = useState<[string, Agg][]>([]);
  const [outboundTop, setOutboundTop] = useState<[string, Agg][]>([]);
  const [ruleTop, setRuleTop] = useState<[string, Agg][]>([]);

  useEffect(() => {
    const un = listen<Snapshot>("connections://tick", (e) => {
      const agg = aggRef.current;
      const add = (map: Map<string, Agg>, key: string, dDown: number, dUp: number) => {
        const cur = map.get(key) ?? { count: 0, down: 0, up: 0 };
        cur.count += 0; // count 只在新连接时 +1，见下
        cur.down += dDown;
        cur.up += dUp;
        map.set(key, cur);
      };

      for (const c of e.payload.connections) {
        const host = c.metadata.host || c.metadata.destinationIP || "未知";
        const outbound = c.chains?.[0] ?? "DIRECT";
        const rule = c.rule
          ? `${c.rule}${c.rulePayload ? ` ${c.rulePayload}` : ""}`
          : "其他";

        const prev = agg.lastSeen.get(c.id) ?? { up: 0, down: 0 };
        const dDown = Math.max(c.download - prev.down, 0);
        const dUp = Math.max(c.upload - prev.up, 0);
        const isNew = !agg.lastSeen.has(c.id);
        agg.lastSeen.set(c.id, { up: c.upload, down: c.download });

        for (const [map, key] of [
          [agg.hosts, host],
          [agg.outbounds, outbound],
          [agg.rules, rule],
        ] as const) {
          add(map, key, dDown, dUp);
          if (isNew) map.get(key)!.count += 1;
        }
      }

      // 快照中消失的连接从 lastSeen 移除（流量已计入聚合）
      const live = new Set(e.payload.connections.map((c) => c.id));
      for (const id of [...agg.lastSeen.keys()]) {
        if (!live.has(id)) agg.lastSeen.delete(id);
      }

      const top = (m: Map<string, Agg>) =>
        [...m.entries()].sort((a, b) => b[1].down + b[1].up - (a[1].down + a[1].up)).slice(0, TOP_N);
      setHostTop(top(agg.hosts));
      setOutboundTop(top(agg.outbounds));
      setRuleTop(top(agg.rules));
    });
    return () => {
      un.then((fn) => fn());
    };
  }, []);

  // 规则统计：内核规则全集按类型计数
  const [ruleTypes, setRuleTypes] = useState<[string, number][]>([]);
  useEffect(() => {
    if (!running) {
      setRuleTypes([]);
      return;
    }
    getRules()
      .then((data) => {
        const rules = (data as { rules?: RuleItem[] }).rules ?? [];
        const byType = new Map<string, number>();
        for (const r of rules) byType.set(r.type, (byType.get(r.type) ?? 0) + 1);
        setRuleTypes([...byType.entries()].sort((a, b) => b[1] - a[1]));
      })
      .catch(() => setRuleTypes([]));
  }, [running]);

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold">分析</h1>
        {!running && (
          <span className="text-xs text-gray-500">内核未运行，仅展示已采集数据</span>
        )}
      </div>

      {/* 聚合统计：实时曲线在仪表盘，本页专注聚合 */}
      <div className="grid grid-cols-1 xl:grid-cols-2 gap-6">
        {/* Top 主机名 */}
        <AggCard
          icon={<Globe2 size={16} className="text-accent" />}
          title="Top 主机名（按流量）"
          rows={hostTop}
          empty="暂无连接数据"
        />
        {/* 策略分布 */}
        <AggCard
          icon={<PieChart size={16} className="text-accent" />}
          title="策略分布（按出站）"
          rows={outboundTop}
          empty="暂无连接数据"
        />
      </div>

      {/* 规则统计 */}
      <div className="grid grid-cols-1 xl:grid-cols-2 gap-6">
        <div className="bg-surface-card rounded-xl p-5 shadow-md">
          <h2 className="text-base font-semibold mb-4 flex items-center gap-2">
            <ListFilter size={16} className="text-accent" />
            规则统计（按类型）
          </h2>
          {ruleTypes.length === 0 ? (
            <div className="py-6 text-center text-sm text-gray-500">
              {running ? "暂无规则" : "内核未运行"}
            </div>
          ) : (
            <div className="flex flex-wrap gap-2">
              {ruleTypes.map(([t, n]) => (
                <span
                  key={t}
                  className="px-3 py-1.5 rounded-lg bg-surface-hover text-xs text-gray-300"
                >
                  {t} <span className="text-accent font-medium">{n}</span>
                </span>
              ))}
            </div>
          )}
        </div>
        <AggCard
          icon={<ListFilter size={16} className="text-accent" />}
          title="规则命中（按流量）"
          rows={ruleTop}
          empty="暂无命中记录"
        />
      </div>
    </div>
  );
}

function AggCard({
  icon,
  title,
  rows,
  empty,
}: {
  icon: React.ReactNode;
  title: string;
  rows: [string, Agg][];
  empty: string;
}) {
  const max = rows.reduce((m, [, a]) => Math.max(m, a.down + a.up), 1);
  return (
    <div className="bg-surface-card rounded-xl p-5 shadow-md">
      <h2 className="text-base font-semibold mb-4 flex items-center gap-2">
        {icon}
        {title}
      </h2>
      {rows.length === 0 ? (
        <div className="py-6 text-center text-sm text-gray-500">{empty}</div>
      ) : (
        <div className="space-y-2.5">
          {rows.map(([key, a]) => {
            const total = a.down + a.up;
            return (
              <div key={key}>
                <div className="flex items-center justify-between text-xs mb-1">
                  <span className="truncate text-gray-300" title={key}>
                    {key}
                  </span>
                  <span className="shrink-0 pl-3 text-gray-500 font-mono">
                    {a.count} 次 · {formatBytes(total)}
                  </span>
                </div>
                <div className="h-1.5 rounded-full bg-white/5 overflow-hidden">
                  <div
                    className="h-full bg-accent/70 rounded-full"
                    style={{ width: `${Math.max((total / max) * 100, 2)}%` }}
                  />
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
