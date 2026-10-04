import { invoke } from "@tauri-apps/api/core";
import type { UserSettings } from "@/stores/core";

export const startCore = () => invoke<void>("start_core");
export const stopCore = () => invoke<void>("stop_core");
export const getCoreStatus = () => invoke("get_core_status");
export const setMode = (mode: string) => invoke("set_mode", { mode });

export const urlTest = (group: string) =>
  invoke("url_test", { req: { group, url: null, timeoutMs: null } });
export const selectProxy = (group: string, name: string) =>
  invoke("select_proxy", { group, name });

export const updateSubscription = (url: string) =>
  invoke("update_subscription", { url });

export interface SubscriptionRecord {
  url: string;
  updatedAt: number;
  name?: string | null;
  /** 是否为当前生效订阅（最近更新的一条） */
  active: boolean;
  /** 节点数（仅活跃订阅有值） */
  nodeCount?: number | null;
}

export const listSubscriptions = () =>
  invoke<SubscriptionRecord[]>("list_subscriptions");

export const deleteSubscription = (url: string) =>
  invoke<boolean>("delete_subscription", { url });

/** 激活 Pro（占位校验：PRISM-XXXX-XXXX-XXXX 格式即通过） */
export const activatePro = (code: string) =>
  invoke<boolean>("activate_pro", { code });

export const getConnections = () => invoke("get_connections");
export const getTrafficStats = () => invoke("get_traffic_stats");
export const getRules = () => invoke("get_rules");
export const getProxyGroups = () => invoke("get_proxy_groups");

export const getSettings = () => invoke<UserSettings>("get_settings");
export const saveSettings = (settings: UserSettings) =>
  invoke("save_settings", { settings });

export interface KernelInfo {
  exists: boolean;
  path: string | null;
  version: string | null;
}

export const getKernelInfo = () => invoke<KernelInfo>("get_kernel_info");
export const ensureKernel = () => invoke<KernelInfo>("ensure_kernel");

/// 内核下载进度事件载荷
export interface KernelDownloadProgress {
  stage: "download" | "extract" | "ready" | "error";
  percent: number;
  message: string;
}
export const KERNEL_DOWNLOAD_EVENT = "kernel-download://progress";
