import { create } from "zustand";

export type CoreStatus = "stopped" | "starting" | "running" | "stopping" | "error";
export type RunMode = "systemProxy" | "tun" | "ruleOnly";
export type OutboundMode = "rule" | "global" | "direct";

export interface UserSettings {
  mixedPort: number;
  allowLan: boolean;
  systemProxy: boolean;
  /** 运行模式（后端权威持久化；表单提交时会被后端以 set_mode 为准覆盖） */
  mode: RunMode;
  autoStart: boolean;
  /** 历史占位字段，真实授权以 pro store 为准 */
  proUnlocked: boolean;
  /** 授权服务地址（为空使用内置默认） */
  licenseServerUrl?: string | null;
  /** 出站模式（规则/全局/直连） */
  outboundMode?: OutboundMode;
  /** 启用 IPv6 */
  ipv6?: boolean;
  /** 阻止 QUIC（UDP/443） */
  blockQuic?: boolean;
  /** 切换策略后关闭现有连接 */
  closeConnectionsOnSwitch?: boolean;
}

interface TrafficPoint {
  time: string;
  up: number;
  down: number;
}

interface CoreState {
  status: CoreStatus;
  mode: RunMode;
  upSpeed: number;
  downSpeed: number;
  memoryUsage: number;
  uptimeSecs: number;
  traffic: TrafficPoint[];
  settings: UserSettings;

  setStatus: (s: CoreStatus) => void;
  setMode: (m: RunMode) => void;
  setTrafficRate: (up: number, down: number) => void;
  setMemoryUsage: (bytes: number) => void;
  setUptime: (secs: number) => void;
  setSettings: (s: UserSettings) => void;
  pushTraffic: (up: number, down: number) => void;
}

// 环形缓冲：仅保留最近 60 个采样点（1 分钟）
const MAX_POINTS = 60;

export const useCoreStore = create<CoreState>((set, get) => ({
  status: "stopped",
  mode: "systemProxy",
  upSpeed: 0,
  downSpeed: 0,
  memoryUsage: 0,
  uptimeSecs: 0,
  traffic: [],
  settings: {
    mixedPort: 2080,
    allowLan: false,
    systemProxy: true,
    mode: "systemProxy",
    autoStart: false,
    proUnlocked: false,
  },

  setStatus: (status) => set({ status }),
  setMode: (mode) => set({ mode }),
  setTrafficRate: (upSpeed, downSpeed) => set({ upSpeed, downSpeed }),
  setMemoryUsage: (memoryUsage) => set({ memoryUsage }),
  setUptime: (uptimeSecs) => set({ uptimeSecs }),
  setSettings: (settings) => set({ settings }),

  pushTraffic: (up, down) => {
    const point: TrafficPoint = {
      time: new Date().toLocaleTimeString("zh-CN", { hour12: false }),
      up,
      down,
    };
    const next = [...get().traffic, point];
    if (next.length > MAX_POINTS) next.shift();
    set({ traffic: next });
  },
}));
