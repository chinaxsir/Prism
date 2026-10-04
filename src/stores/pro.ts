import { create } from "zustand";

import { activatePro, getSettings } from "@/api/ipc";

interface ProState {
  unlocked: boolean;
  /** App 启动时从设置中读取 proUnlocked */
  init: () => Promise<void>;
  /** 激活 Pro，成功返回 true 并置 unlocked */
  activate: (code: string) => Promise<boolean>;
}

export const useProStore = create<ProState>((set) => ({
  unlocked: false,

  init: async () => {
    try {
      const settings = await getSettings();
      set({ unlocked: settings.proUnlocked });
    } catch (e) {
      console.error("init pro state failed:", e);
    }
  },

  activate: async (code) => {
    // TODO(商业化)：替换为真实内购（StoreKit / Google Play Billing）
    try {
      const ok = await activatePro(code);
      if (ok) set({ unlocked: true });
      return ok;
    } catch (e) {
      console.error("activate pro failed:", e);
      return false;
    }
  },
}));
