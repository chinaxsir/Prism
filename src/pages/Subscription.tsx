import { useCallback, useEffect, useState } from "react";
import { RefreshCw, Rss, Trash2 } from "lucide-react";

import {
  deleteSubscription,
  listSubscriptions,
  toggleSubscription,
  updateSubscription,
  type SubscriptionRecord,
} from "@/api/ipc";
import { ActivateProModal } from "@/components/ProGate";
import EmptyState from "@/components/ui/EmptyState";
import Spinner from "@/components/ui/Spinner";
import { toast } from "@/components/ui/Toast";
import { useProStore } from "@/stores/pro";
import { formatRelativeTime } from "@/utils/time";

function displayName(record: SubscriptionRecord): string {
  if (record.name) return record.name;
  try {
    return new URL(record.url).hostname;
  } catch {
    return record.url;
  }
}

export default function Subscription() {
  const [subs, setSubs] = useState<SubscriptionRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [url, setUrl] = useState("");
  const [adding, setAdding] = useState(false);
  const [updatingUrl, setUpdatingUrl] = useState<string | null>(null);
  const [confirmUrl, setConfirmUrl] = useState<string | null>(null);
  const [showActivate, setShowActivate] = useState(false);
  const unlocked = useProStore((s) => s.unlocked);

  const load = useCallback(async () => {
    try {
      const records = await listSubscriptions();
      setSubs(records ?? []);
    } catch (e) {
      console.error("load subscriptions failed:", e);
      toast.error(`加载订阅列表失败：${String(e)}`);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const handleAdd = async () => {
    const trimmed = url.trim();
    if (!/^https?:\/\/.+/.test(trimmed)) {
      toast.error("请输入有效的 http/https 订阅链接");
      return;
    }
    // 免费版仅支持单订阅：第二条起需要 Pro（全平台门控）
    if (!unlocked && subs.length >= 1) {
      setShowActivate(true);
      return;
    }
    setAdding(true);
    try {
      await updateSubscription(trimmed);
      toast.success("订阅已添加并更新");
      setUrl("");
      await load();
    } catch (e) {
      console.error("add subscription failed:", e);
      toast.error(String(e));
    } finally {
      setAdding(false);
    }
  };

  const handleUpdate = async (record: SubscriptionRecord) => {
    if (updatingUrl) return;
    setUpdatingUrl(record.url);
    try {
      await updateSubscription(record.url);
      toast.success("订阅已更新");
      await load();
    } catch (e) {
      console.error("update subscription failed:", e);
      toast.error(String(e));
    } finally {
      setUpdatingUrl(null);
    }
  };

  const handleToggle = async (record: SubscriptionRecord) => {
    try {
      await toggleSubscription(record.url, !record.enabled);
      toast.success(record.enabled ? "订阅已停用" : "订阅已启用");
      await load();
    } catch (e) {
      console.error("toggle subscription failed:", e);
      toast.error(String(e));
    }
  };

  const handleDelete = async (record: SubscriptionRecord) => {
    try {
      await deleteSubscription(record.url);
      toast.success("订阅已删除");
      setConfirmUrl(null);
      await load();
    } catch (e) {
      console.error("delete subscription failed:", e);
      toast.error(String(e));
    }
  };

  return (
    <div className="space-y-6">
      <h1 className="text-2xl font-bold">订阅</h1>

      {/* 添加订阅 */}
      <div className="bg-surface-card rounded-xl p-5">
        <h2 className="text-base font-semibold mb-4">添加订阅</h2>
        <div className="flex flex-col gap-3 md:flex-row">
          <input
            type="text"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="输入订阅 URL（http/https）"
            className="flex-1 px-4 py-2 rounded-lg bg-surface-hover border border-white/5 focus:border-accent outline-none text-sm"
          />
          <button
            onClick={handleAdd}
            disabled={adding || !url.trim()}
            className="flex items-center justify-center gap-2 px-6 py-2 rounded-lg bg-accent hover:bg-accent-hover transition-colors text-sm disabled:opacity-50"
          >
            {adding && <Spinner size={14} className="text-white" />}
            {adding ? "添加中…" : "添加并更新"}
          </button>
        </div>
      </div>

      {/* 订阅列表 */}
      <div className="bg-surface-card rounded-xl p-5">
        <h2 className="text-base font-semibold mb-4">
          我的订阅（{subs.length}）
        </h2>

        {loading ? (
          <div className="flex justify-center py-10">
            <Spinner />
          </div>
        ) : subs.length === 0 ? (
          <EmptyState
            icon={<Rss size={32} />}
            title="暂无订阅"
            description="添加订阅链接后即可拉取节点，免费版支持单订阅，Pro 支持多订阅"
          />
        ) : (
          <div className="space-y-3">
            {subs.map((record) => (
              <div
                key={record.url}
                className="flex flex-col gap-3 rounded-lg bg-surface-hover p-4 md:flex-row md:items-center"
              >
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-sm font-medium">
                      {displayName(record)}
                    </span>
                    {record.enabled && (
                      <span className="shrink-0 rounded bg-accent/20 px-1.5 py-0.5 text-[10px] text-accent">
                        已启用
                      </span>
                    )}
                  </div>
                  <div className="mt-1 truncate font-mono text-[11px] text-gray-500">
                    {record.url}
                  </div>
                  <div className="mt-1 text-[11px] text-gray-500">
                    更新于 {formatRelativeTime(record.updatedAt)}
                    {record.nodeCount != null && (
                      <span className="ml-2">{record.nodeCount} 个节点</span>
                    )}
                  </div>
                </div>

                <div className="flex shrink-0 items-center gap-2">
                  {/* 启停开关：停用后该订阅不参与配置合并 */}
                  <button
                    onClick={() => handleToggle(record)}
                    className={`relative h-5 w-9 rounded-full transition-colors ${
                      record.enabled ? "bg-accent" : "bg-white/10"
                    }`}
                    title={record.enabled ? "点击停用" : "点击启用"}
                  >
                    <span
                      className={`absolute top-0.5 h-4 w-4 rounded-full bg-white transition-transform ${
                        record.enabled ? "translate-x-[18px]" : "translate-x-0.5"
                      }`}
                    />
                  </button>
                  <button
                    onClick={() => handleUpdate(record)}
                    disabled={updatingUrl === record.url}
                    className="flex items-center gap-1.5 rounded-lg bg-surface-card px-3 py-1.5 text-xs text-gray-300 hover:text-gray-100 disabled:opacity-50"
                  >
                    {updatingUrl === record.url ? (
                      <Spinner size={12} />
                    ) : (
                      <RefreshCw size={12} />
                    )}
                    更新
                  </button>

                  {confirmUrl === record.url ? (
                    <>
                      <button
                        onClick={() => handleDelete(record)}
                        className="rounded-lg bg-latency-bad/20 px-3 py-1.5 text-xs text-latency-bad hover:bg-latency-bad/30"
                      >
                        确认删除
                      </button>
                      <button
                        onClick={() => setConfirmUrl(null)}
                        className="rounded-lg bg-surface-card px-3 py-1.5 text-xs text-gray-400 hover:text-gray-200"
                      >
                        取消
                      </button>
                    </>
                  ) : (
                    <button
                      onClick={() => setConfirmUrl(record.url)}
                      className="flex items-center gap-1.5 rounded-lg bg-surface-card px-3 py-1.5 text-xs text-gray-400 hover:text-latency-bad"
                    >
                      <Trash2 size={12} />
                      删除
                    </button>
                  )}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      <ActivateProModal
        open={showActivate}
        onClose={() => setShowActivate(false)}
      />
    </div>
  );
}
