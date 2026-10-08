import { useEffect, useState } from "react";
import { Zap, Gauge, RefreshCw, ChevronRight, Search } from "lucide-react";
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
  const [mainSelectorName, setMainSelectorName] = useState("");
  const [current, setCurrent] = useState<string>("");
  const [nodes, setNodes] = useState<ProxyEntry[]>([]);
  const [testing, setTesting] = useState(false);
  /// 上一轮组测速中失败的节点（显示为「超时」）
  const [failed, setFailed] = useState<Set<string>>(new Set());
  /// 最新一轮测速结果（用于实时刷新延迟显示，避免等 loadProxies 往返）
  const [delays, setDelays] = useState<Record<string, number>>({});
  /// 用户手动选择标记：一旦用户点选过某节点，不再自动切换到最低延迟
  const [userTouched, setUserTouched] = useState(false);
  const [keyword, setKeyword] = useState("");

  useEffect(() => {
    getCoreStatus()
      .then((d) => {
        setMainSelectorName(d.mainSelector ?? "");
        loadProxies(d.mainSelector ?? "");
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

      const groupName = preferred || mainSelectorName;
      const group = map[groupName];
      if (!group) return;

      const all = (group.all ?? [])
        .map((item) => (typeof item === "string" ? map[item] : item))
        .filter((item): item is ProxyEntry => Boolean(item))
        .filter((item) => !GROUP_TYPES.includes(item.type));
      setNodes(all);
      setCurrent(group.now ?? "");

      // 未选默认延迟最低：now 为空或不在节点列表中，且用户未手动操作过
      const nowValid = group.now && all.some((n) => n.name === group.now);
      if (!nowValid && !userTouched && all.length > 0) {
        // 先测速再选最快
        autoSelectFastest(groupName);
      }
    } catch (e) {
      console.error("load proxies failed:", e);
    }
  };

  const autoSelectFastest = async (groupName: string) => {
    try {
      const result = (await urlTest(groupName)) as Record<
        string,
        number
      > | null;
      if (!result || Object.keys(result).length === 0) return;
      const entries = Object.entries(result).filter(
        ([, d]) => d > 0
      );
      if (entries.length === 0) return;
      entries.sort((a, b) => a[1] - b[1]);
      const fastest = entries[0][0];
      if (fastest) {
        await selectProxy(groupName, fastest);
        setCurrent(fastest);
        toast.info(`已自动选择延迟最低节点：${fastest}`);
        await loadProxies();
      }
    } catch (e) {
      console.error("auto select failed:", e);
    }
  };

  const lastDelay = (node: ProxyEntry): Delay => {
    if (failed.has(node.name)) return 0;
    // 优先用最新测速结果
    if (node.name in delays) return delays[node.name];
    const history = node.history ?? [];
    if (history.length === 0) return undefined;
    return history[history.length - 1].delay;
  };

  const handleTest = async () => {
    if (!mainSelectorName || testing) return;
    if (nodes.length === 0) {
      toast.error("当前没有可测速的节点");
      return;
    }
    setTesting(true);
    try {
      const result = (await urlTest(mainSelectorName)) as Record<
        string,
        number
      > | null;
      const ok = new Set(Object.keys(result ?? {}));
      setFailed(
        new Set(nodes.map((n) => n.name).filter((n) => !ok.has(n)))
      );
      setDelays(result ?? {});
    } catch (e) {
      console.error("url test failed:", e);
      toast.error(`测速失败：${String(e)}`);
    } finally {
      setTesting(false);
    }
  };

  const handleSelect = async (name: string) => {
    if (current === name) return;
    setUserTouched(true);
    try {
      await selectProxy(mainSelectorName, name);
      setCurrent(name);
      await loadProxies();
    } catch (e) {
      console.error("select proxy failed:", e);
    }
  };

  const filtered = keyword.trim()
    ? nodes.filter((n) =>
        n.name.toLowerCase().includes(keyword.trim().toLowerCase())
      )
    : nodes;

  if (nodes.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-white/10 bg-surface-card/50 py-16 px-6 text-center">
        <div className="mb-4 flex h-14 w-14 items-center justify-center rounded-2xl bg-accent/10 text-accent">
          <Gauge size={26} />
        </div>
        <h3 className="text-base font-semibold text-gray-200">
          {coreStatus === "running" ? "暂无节点" : "内核未运行"}
        </h3>
        <p className="mt-2 max-w-sm text-sm leading-relaxed text-gray-500">
          {coreStatus === "running"
            ? "当前订阅没有可展示的节点，请尝试更新订阅。"
            : "请在仪表盘点击「启动」，启动后将自动加载订阅中的节点，可在此测速、切换节点。"}
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
      {/* 顶部工具栏：搜索 + 测速 */}
      <div className="flex items-center gap-2">
        <div className="flex-1 flex items-center gap-2 px-3 py-2 rounded-lg bg-surface-card border border-white/5">
          <Search size={14} className="text-gray-500" />
          <input
            value={keyword}
            onChange={(e) => setKeyword(e.target.value)}
            placeholder="搜索节点…"
            className="flex-1 bg-transparent text-sm outline-none placeholder:text-gray-600"
          />
        </div>
        <button
          onClick={handleTest}
          disabled={testing}
          className="shrink-0 flex items-center gap-1.5 px-3.5 py-2 rounded-lg bg-accent text-white text-xs font-medium disabled:opacity-50"
        >
          <Zap size={13} className={testing ? "animate-pulse" : ""} />
          {testing ? "测速中" : "测速"}
        </button>
      </div>

      {/* 当前选中节点信息 */}
      <div className="flex items-center justify-between px-1 text-xs text-gray-400">
        <span>
          当前：
          <span className="text-accent">{current || "未选择"}</span>
        </span>
        <span>{filtered.length} 节点</span>
      </div>

      {/* 节点列表流（Shadowrocket 风格） */}
      <div className="rounded-2xl bg-surface-card overflow-hidden border border-white/5">
        {filtered.map((node, idx) => {
          const delay = lastDelay(node);
          const selected = current === node.name;
          return (
            <button
              key={node.name}
              onClick={() => handleSelect(node.name)}
              className={clsx(
                "w-full flex items-center gap-3 px-4 py-3 text-left transition-colors",
                selected ? "bg-accent/10" : "hover:bg-white/[0.03]",
                idx !== filtered.length - 1 && "border-b border-white/5"
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

              <ChevronRight size={14} className="shrink-0 text-gray-600" />
            </button>
          );
        })}
      </div>
    </div>
  );
}
