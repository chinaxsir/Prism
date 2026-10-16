import { create } from "zustand";

import {
  activatePro,
  deactivatePro,
  getEntitlement,
  refreshEntitlement,
  type Entitlement,
} from "@/api/ipc";

const INACTIVE: Entitlement = {
  status: "inactive",
  kind: null,
  expiresAt: null,
  source: null,
  lastVerifiedAt: 0,
};

/** 可使用 Pro：active（联网有效）或 grace（离线宽限） */
const unlockedOf = (e: Entitlement) =>
  e.status === "active" || e.status === "grace";

interface ProState {
  entitlement: Entitlement;
  /** 首次状态拉取完成（避免启动瞬间误弹 Pro 窗） */
  ready: boolean;
  unlocked: boolean;
  busy: boolean;
  error: string | null;

  init: () => Promise<void>;
  setEntitlement: (e: Entitlement) => void;
  /** email 可选：部分 License Key 激活时需邮箱校验 */
  activate: (licenseKey: string, email?: string) => Promise<boolean>;
  refresh: () => Promise<void>;
  deactivate: () => Promise<void>;
}

export const useProStore = create<ProState>((set) => ({
  entitlement: INACTIVE,
  ready: false,
  unlocked: false,
  busy: false,
  error: null,

  init: async () => {
    try {
      const e = await getEntitlement();
      set({ entitlement: e, unlocked: unlockedOf(e), ready: true });
    } catch (err) {
      console.error("get entitlement failed:", err);
      set({ ready: true });
    }
  },

  setEntitlement: (e) =>
    set({ entitlement: e, unlocked: unlockedOf(e), error: null }),

  activate: async (licenseKey, email) => {
    set({ busy: true, error: null });
    try {
      // ipc.ts 的 activatePro 正在并行改造为 (licenseKey, email?)；
      // 此处 email 兜底为空串，与旧签名 (code, email) 及新签名均兼容。
      const e = await activatePro(licenseKey, email ?? "");
      set({ entitlement: e, unlocked: unlockedOf(e), busy: false });
      return unlockedOf(e);
    } catch (err) {
      // 保留完整错误文本（429 限流时含 retry_after，UI 需解析）
      const message =
        typeof err === "string"
          ? err
          : err instanceof Error
            ? err.message
            : String(err);
      set({ busy: false, error: message || "激活失败" });
      return false;
    }
  },

  refresh: async () => {
    set({ busy: true, error: null });
    try {
      const e = await refreshEntitlement();
      set({ entitlement: e, unlocked: unlockedOf(e), busy: false });
    } catch (err) {
      set({ busy: false, error: String(err) });
    }
  },

  deactivate: async () => {
    const e = await deactivatePro();
    set({ entitlement: e, unlocked: unlockedOf(e), error: null });
  },
}));
