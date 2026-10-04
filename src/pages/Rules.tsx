import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";

import { getRules, listSubscriptions, updateSubscription } from "@/api/ipc";

interface Rule {
  type: string;
  payload: string;
  proxy?: string;
  outbound?: string;
}

export default function Rules() {
  const [rules, setRules] = useState<Rule[]>([]);
  const [loading, setLoading] = useState(false);
  const [subscriptionUrl, setSubscriptionUrl] = useState("");
  const [updating, setUpdating] = useState(false);
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(
    null
  );

  useEffect(() => {
    loadRules();
    // 回填上次保存的订阅 URL（持久化于 subscriptions.json）
    listSubscriptions()
      .then((records) => {
        if (records?.length && records[0].url) {
          setSubscriptionUrl(records[0].url);
        }
      })
      .catch((e) => console.error("load subscriptions failed:", e));
  }, []);

  const loadRules = async () => {
    setLoading(true);
    try {
      const resp = (await getRules()) as { rules?: Rule[] };
      setRules(resp.rules ?? []);
    } catch (e) {
      console.error("load rules failed:", e);
    } finally {
      setLoading(false);
    }
  };

  const handleUpdateSubscription = async () => {
    if (!subscriptionUrl || updating) return;
    setUpdating(true);
    setFeedback(null);
    try {
      await updateSubscription(subscriptionUrl);
      setFeedback({ ok: true, text: "订阅已更新（重启内核后生效）" });
      await loadRules();
    } catch (e) {
      console.error("update subscription failed:", e);
      setFeedback({ ok: false, text: String(e) });
    } finally {
      setUpdating(false);
    }
  };

  const ruleTypeClass = (type: string) => {
    const colors: Record<string, string> = {
      DOMAIN: "text-blue-400",
      "DOMAIN-SUFFIX": "text-green-400",
      "DOMAIN-KEYWORD": "text-yellow-400",
      "IP-CIDR": "text-purple-400",
      GEOIP: "text-pink-400",
      GEOSITE: "text-orange-400",
      "PROCESS-NAME": "text-red-400",
      RULESET: "text-cyan-400",
    };
    return colors[type] ?? "text-gray-400";
  };

  return (
    <div className="space-y-6">
      <h1 className="text-2xl font-bold">规则</h1>

      {/* 订阅管理 */}
      <div className="bg-surface-card rounded-xl p-5">
        <h2 className="text-base font-semibold mb-4">订阅管理</h2>
        <div className="flex gap-3">
          <input
            type="text"
            value={subscriptionUrl}
            onChange={(e) => setSubscriptionUrl(e.target.value)}
            placeholder="输入订阅 URL（保存后下次更新生效）"
            className="flex-1 px-4 py-2 rounded-lg bg-surface-hover border border-white/5 focus:border-accent outline-none text-sm"
          />
          <button
            onClick={handleUpdateSubscription}
            disabled={updating}
            className="px-6 py-2 rounded-lg bg-accent hover:bg-accent-hover transition-colors text-sm disabled:opacity-50"
          >
            {updating ? "更新中…" : "更新订阅"}
          </button>
        </div>
        {feedback && (
          <p
            className={`mt-2 text-xs ${
              feedback.ok ? "text-green-400" : "text-red-400"
            }`}
          >
            {feedback.text}
          </p>
        )}
      </div>

      {/* 规则列表 */}
      <div className="bg-surface-card rounded-xl p-5">
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-base font-semibold">
            当前规则（{rules.length} 条，自上而下优先匹配）
          </h2>
          <button
            onClick={loadRules}
            className="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-surface-hover text-xs text-gray-400 hover:text-gray-200"
          >
            <RefreshCw size={13} className={loading ? "animate-spin" : ""} />
            刷新
          </button>
        </div>

        {rules.length === 0 ? (
          <p className="text-gray-500 text-sm py-8 text-center">
            {loading ? "加载中…" : "暂无规则（内核未启动）"}
          </p>
        ) : (
          <div className="space-y-1.5 max-h-[28rem] overflow-y-auto pr-1">
            {rules.map((rule, index) => (
              <div
                key={`${rule.type}-${rule.payload}-${index}`}
                className="flex items-center gap-3 p-2.5 rounded-lg bg-surface-hover"
              >
                <span className="w-8 text-right text-[11px] text-gray-600 font-mono">
                  {index + 1}
                </span>
                <span
                  className={`w-32 text-[11px] font-mono ${ruleTypeClass(
                    rule.type
                  )}`}
                >
                  {rule.type}
                </span>
                <span className="flex-1 text-xs font-mono truncate">
                  {rule.payload}
                </span>
                <span className="text-[11px] text-accent">
                  → {rule.proxy ?? rule.outbound ?? "—"}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
