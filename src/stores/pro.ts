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
  activate: (code: string, email: string) => Promise<boolean>;
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

  activate: async (code, email) => {
    set({ busy: true, error: null });
    try {
      const e = await activatePro(code, email);
      set({ entitlement: e, unlocked: unlockedOf(e), busy: false });
      return unlockedOf(e);
    } catch (err) {
      const message = typeof err === "string" ? err : "激活失败";
      set({ busy: false, error: message });
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
