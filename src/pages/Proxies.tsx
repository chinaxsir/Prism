import { useEffect, useState } from "react";
import { Zap, Gauge, RefreshCw, ChevronRight } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { Link } from "react-router-dom";

import {
  getCoreStatus,
  getProxyGroups,
  selectProxy,
  urlTest,
} from "@/api/ipc";
import { useCoreStore } from "@/stores/core";
import { toast } from "@/components/ui/Toast";
import { delayClass, delayText, type Delay } from "@/utils/latency";
import clsx from "clsx";

interface DelaySample {
  delay: number;
}

interface ProxyEntry {
  name: string;
  type: string;
  now?: string;
  all?: Array<ProxyEntry | string>;
  history?: DelaySample[];
}

const GROUP_TYPES = ["Selector", "URLTest", "Fallback", "LoadBalance"];

const GROUP_TYPE_LABEL: Record<string, string> = {
  Selector: "手动选择",
  URLTest: "自动测速",
  Fallback: "故障转移",
  LoadBalance: "负载均衡",
};

/// 节点协议显示标签（与 sing-box outbound type 对齐）
const PROTOCOL_LABEL: Record<string, string> = {
  Vless: "VLESS",
  VMess: "VMess",
  Trojan: "Trojan",
  Hysteria: "HY",
  Hysteria2: "HY2",
  TUIC: "TUIC",
  Shadowsocks: "SS",
  ShadowsocksR: "SSR",
  WireGuard: "WG",
  Socks: "SOCKS",
  HTTP: "HTTP",
  Direct: "直连",
  Block: "阻断",
};

const protocolTag = (type: string): string =>
  PROTOCOL_LABEL[type] ?? type ?? "—";

export default function Proxies() {
  const coreStatus = useCoreStore((s) => s.status);
  const [proxiesMap, setProxiesMap] = useState<Record<string, ProxyEntry>>(
    {}
  );
  const [groups, setGroups] = useState<ProxyEntry[]>([]);
  const [activeGroup, setActiveGroup] = useState("");
  const [testing, setTesting] = useState(false);
  /// 上一轮组测速中失败（未出现在结果 map）的节点，展示为「超时」
  const [failed, setFailed] = useState<Set<string>>(new Set());
  /// 主选择组（route.final 链上最深的 selector）：默认落在此 tab，选择才真实生效
  const [mainSelector, setMainSelector] = useState("");

  useEffect(() => {
    getCoreStatus()
      .then((d) => {
        const ms = d.mainSelector ?? "";
        setMainSelector(ms);
        loadProxies(ms);
      })
      .catch(() => loadProxies());

    const unStatus = listen("core://status", () => loadProxies());
    const unProxies = listen("proxies::changed", () => loadProxies());
    return () => {
      unStatus.then((fn) => fn());
      unProxies.then((fn) => fn());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const loadProxies = async (preferred?: string) => {
    try {
      const resp = (await getProxyGroups()) as {
        proxies?: Record<string, ProxyEntry>;
      };
      const map = resp.proxies ?? {};
      setProxiesMap(map);

      const groupList = Object.values(map).filter(
        (g) => GROUP_TYPES.includes(g.type) && g.name !== "GLOBAL"
      );
      setGroups(groupList);

      setActiveGroup(
        (prev) =>
          (prev && map[prev] && prev) ||
          (preferred && map[preferred] && preferred) ||
          (mainSelector && map[mainSelector] && mainSelector) ||
          groupList[0]?.name ||
          ""
      );
    } catch (e) {
      console.error("load proxies failed:", e);
    }
  };

  const current = groups.find((g) => g.name === activeGroup);

  const nodes: ProxyEntry[] = (current?.all ?? [])
    .map((item) => (typeof item === "string" ? proxiesMap[item] : item))
    .filter((item): item is ProxyEntry => Boolean(item))
    .filter((item) => !GROUP_TYPES.includes(item.type));

  /// 0 = 超时；undefined = 未测速
  const lastDelay = (node: ProxyEntry): Delay => {
    if (failed.has(node.name)) return 0;
    const history = node.history ?? [];
    if (history.length === 0) return undefined;
    return history[history.length - 1].delay;
  };

  const handleTestGroup = async () => {
    if (!activeGroup || testing) return;
    if (nodes.length === 0) {
      toast.error("当前策略组没有可测速的节点");
      return;
    }
    setTesting(true);
    try {
      const result = (await urlTest(activeGroup)) as Record<
        string,
        number
      > | null;
      const ok = new Set(Object.keys(result ?? {}));
      setFailed(
        new Set(nodes.map((n) => n.name).filter((n) => !ok.has(n)))
      );
      await loadProxies();
    } catch (e) {
      console.error("url test failed:", e);
      toast.error(`测速失败：${String(e)}`);
    } finally {
      setTesting(false);
    }
  };

  const handleSelect = async (name: string) => {
    if (current?.now === name) return;
    try {
      await selectProxy(activeGroup, name);
      await loadProxies();
    } catch (e) {
      console.error("select proxy failed:", e);
    }
  };

  if (groups.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-white/10 bg-surface-card/50 py-16 px-6 text-center">
        <div className="mb-4 flex h-14 w-14 items-center justify-center rounded-2xl bg-accent/10 text-accent">
          <Gauge size={26} />
        </div>
        <h3 className="text-base font-semibold text-gray-200">
          {coreStatus === "running" ? "暂无策略组" : "内核未运行"}
        </h3>
        <p className="mt-2 max-w-sm text-sm leading-relaxed text-gray-500">
          {coreStatus === "running"
            ? "当前订阅没有可展示的策略组，请尝试更新订阅。"
            : "请在仪表盘点击「启动」，启动后将自动加载订阅中的节点与策略组，可在此测速、切换节点。"}
        </p>
        <div className="mt-6 flex items-center gap-3">
          {coreStatus !== "running" && (
            <Link
              to="/dashboard"
              className="flex items-center gap-2 rounded-lg bg-accent px-5 py-2.5 text-sm font-medium text-white transition-colors hover:bg-accent-hover"
            >
              <Gauge size={15} />
              前往仪表盘启动
            </Link>
          )}
          <Link
            to="/subscription"
            className="flex items-center gap-2 rounded-lg border border-white/10 px-5 py-2.5 text-sm text-gray-300 transition-colors hover:border-white/20"
          >
            <RefreshCw size={14} />
            管理订阅
          </Link>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {/* 顶部策略组横向 chips */}
      <div className="flex items-center gap-2 overflow-x-auto pb-1 -mx-1 px-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
        {groups.map((g) => (
          <button
            key={g.name}
            onClick={() => {
              setActiveGroup(g.name);
              setFailed(new Set());
            }}
            className={clsx(
              "shrink-0 px-3.5 py-1.5 rounded-full text-xs font-medium transition-colors",
              g.name === activeGroup
                ? "bg-accent text-white"
                : "bg-surface-card text-gray-400 hover:text-gray-200 border border-white/5"
            )}
          >
            {g.name}
          </button>
        ))}
        <div className="shrink-0 w-px h-5 bg-white/10 mx-1" />
        <button
          onClick={handleTestGroup}
          disabled={testing}
          className="shrink-0 flex items-center gap-1.5 px-3.5 py-1.5 rounded-full text-xs font-medium bg-surface-card border border-white/5 hover:border-white/15 disabled:opacity-50"
        >
          <Zap size={13} className={testing ? "animate-pulse" : ""} />
          {testing ? "测速中" : "测速"}
        </button>
      </div>

      {/* 当前组信息条 */}
      {current && (
        <div className="flex items-center justify-between px-1">
          <div className="flex items-center gap-2 text-xs text-gray-400">
            <span className="text-gray-200 text-sm font-medium">
              {current.name}
            </span>
            <span className="text-gray-600">·</span>
            <span>{GROUP_TYPE_LABEL[current.type] ?? current.type}</span>
            <span className="text-gray-600">·</span>
            <span>
              当前：
              <span className="text-accent">{current.now ?? "—"}</span>
            </span>
            <span className="text-gray-600">·</span>
            <span>{nodes.length} 节点</span>
          </div>
        </div>
      )}

      {/* 节点列表流（Shadowrocket 风格） */}
      {current && (
        <div className="rounded-2xl bg-surface-card overflow-hidden border border-white/5">
          {nodes.map((node, idx) => {
            const delay = lastDelay(node);
            const selected = current.now === node.name;
            return (
              <button
                key={node.name}
                onClick={() => handleSelect(node.name)}
                className={clsx(
                  "w-full flex items-center gap-3 px-4 py-3 text-left transition-colors",
                  selected ? "bg-accent/10" : "hover:bg-white/[0.03]",
                  idx !== nodes.length - 1 && "border-b border-white/5"
                )}
              >
                {/* 选中指示器 */}
                <span
                  className={clsx(
                    "shrink-0 w-2 h-2 rounded-full transition-colors",
                    selected ? "bg-accent" : "bg-transparent"
                  )}
                />

                {/* 节点名 */}
                <span
                  className={clsx(
                    "flex-1 truncate text-sm",
                    selected ? "text-gray-100 font-medium" : "text-gray-300"
                  )}
                >
                  {node.name}
                </span>

                {/* 协议标签 */}
                <span className="shrink-0 px-1.5 py-0.5 rounded text-[10px] font-mono uppercase tracking-wide bg-white/5 text-gray-400">
                  {protocolTag(node.type)}
                </span>

                {/* 延迟 */}
                <span
                  className={clsx(
                    "shrink-0 w-14 text-right text-xs font-mono",
                    delayClass(delay)
                  )}
                >
                  {delayText(delay)}
                </span>

                <ChevronRight
                  size={14}
                  className="shrink-0 text-gray-600"
                />
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
