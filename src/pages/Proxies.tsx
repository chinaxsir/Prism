import { useEffect, useState } from "react";
import { Zap, Gauge, RefreshCw } from "lucide-react";
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
    // 先取主选择组再加载节点，保证默认 tab 落在流量实际经过的选择组
    getCoreStatus()
      .then((d) => {
        const ms = d.mainSelector ?? "";
        setMainSelector(ms);
        loadProxies(ms);
      })
      .catch(() => loadProxies());

    // 内核状态变化（启动/停止/重启）时刷新节点列表：clash API 仅在内核运行时可用
    const unStatus = listen("core://status", () => loadProxies());
    // 内核启动/模式切换后的自动选点完成时刷新
    const unProxies = listen("proxies://changed", () => loadProxies());
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

      // GLOBAL 是内核自动生成的虚拟组，不在路由链路中：在其中选节点不会生效，直接隐藏
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

  // 组内节点（all 中可能是对象或节点名字符串）
  // 只显示真实节点（proxiesMap 中 type 非 Selector/URLTest/Fallback/LoadBalance 的条目），
  // 过滤掉嵌套的策略组引用——否则节点页会把策略组当成节点展示，造成「节点数偏少」的观感。
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
      // 后端逐节点测速后返回 {name: delay} 的 map；缺席成员标记为超时
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

  return (
    <div className="space-y-6">
      {groups.length === 0 ? (
        <div className="flex flex-col items-center justify-center rounded-xl border border-dashed border-white/10 bg-surface-card/50 py-20 px-6 text-center">
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
      ) : (
        <>
      {/* 策略组标签页 */}
      <div className="flex flex-wrap items-center gap-2">
        {groups.map((g) => (
          <button
            key={g.name}
            onClick={() => {
                setActiveGroup(g.name);
                setFailed(new Set());
              }}
            className={clsx(
              "px-4 py-2 rounded-lg text-sm transition-colors",
              g.name === activeGroup
                ? "bg-accent text-white"
                : "bg-surface-card text-gray-400 hover:text-gray-200"
            )}
          >
            {g.name}
          </button>
        ))}
      </div>

      {current && (
        <div className="bg-surface-card rounded-xl p-5">
          <div className="flex items-center justify-between mb-5">
            <div>
              <h2 className="text-lg font-semibold">{current.name}</h2>
              <p className="text-xs text-gray-400 mt-1">
                {GROUP_TYPE_LABEL[current.type] ?? current.type}
                <span className="mx-2">·</span>
                当前：<span className="text-accent">{current.now ?? "—"}</span>
                <span className="mx-2">·</span>
                {nodes.length} 个节点
              </p>
            </div>
            <button
              onClick={handleTestGroup}
              disabled={testing}
              className="flex items-center gap-2 px-4 py-2 rounded-lg bg-surface-hover hover:bg-surface-hover text-sm disabled:opacity-50"
            >
              <Zap size={15} className={testing ? "animate-pulse" : ""} />
              {testing ? "测速中…" : "组内测速"}
            </button>
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 xl:grid-cols-4 gap-3">
            {nodes.map((node) => {
              const delay = lastDelay(node);
              const selected = current.now === node.name;

              return (
                <button
                  key={node.name}
                  onClick={() => handleSelect(node.name)}
                  className={clsx(
                    "text-left p-3 rounded-lg border transition-all",
                    selected
                      ? "border-accent bg-accent/10"
                      : "border-white/5 hover:border-white/15"
                  )}
                >
                  <div className="text-sm font-medium truncate">
                    {node.name}
                  </div>
                  <div className="flex items-center justify-between mt-1">
                    <span className="text-[11px] text-gray-500">
                      {node.type}
                    </span>
                    <span
                      className={clsx("text-[11px] font-mono", delayClass(delay))}
                    >
                      {delayText(delay)}
                    </span>
                  </div>
                </button>
              );
            })}
          </div>
        </div>
      )}
        </>
      )}
    </div>
  );
}
