import { useEffect } from "react";
import { Routes, Route, Navigate } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import Sidebar from "@/components/Sidebar";
import TitleBar from "@/components/TitleBar";
import BottomNav from "@/components/BottomNav";
import ToastViewport from "@/components/ui/Toast";
import Dashboard from "@/pages/Dashboard";
import Subscription from "@/pages/Subscription";
import Analytics from "@/pages/Analytics";
import Settings from "@/pages/Settings";
import { useCoreStore, type CoreStatus, type RunMode } from "@/stores/core";
import { useProStore } from "@/stores/pro";
import type { Entitlement } from "@/api/ipc";
import ProxiesHub from "@/pages/ProxiesHub";

export default function App() {
  const initPro = useProStore((s) => s.init);

  // 启动时读取授权状态（命令层从 license 缓存恢复）
  useEffect(() => {
    initPro();
  }, [initPro]);

  // 授权状态广播（启动后台校验 / 激活 / 解绑后，Rust 侧推送）
  useEffect(() => {
    const unlisten = listen<Entitlement>("pro://status", (event) => {
      useProStore.getState().setEntitlement(event.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // 监听内核状态广播：订阅增删改/设置变更触发的 restart_core
  // 对前端透明，重启成功或失败转 Error 都能即时对齐，无需轮询
  useEffect(() => {
    const unlisten = listen<{
      status: CoreStatus;
      mode: RunMode;
      uptimeSecs: number;
    }>("core://status", (event) => {
      const { status, mode, uptimeSecs } = event.payload;
      const store = useCoreStore.getState();
      store.setStatus(status);
      store.setMode(mode);
      store.setUptime(uptimeSecs);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  return (
    <div className="flex flex-col h-screen bg-surface text-gray-200 overflow-hidden">
      <TitleBar />
      <div className="flex flex-1 overflow-hidden">
        <Sidebar />
        <main className="flex-1 overflow-y-auto p-6 pb-16 md:pb-6">
          <Routes>
            <Route path="/" element={<Navigate to="/dashboard" replace />} />
            <Route path="/dashboard" element={<Dashboard />} />
            <Route path="/proxies" element={<ProxiesHub />} />
            <Route path="/subscription" element={<Subscription />} />
            <Route path="/rules" element={<Navigate to="/proxies?tab=rules" replace />} />
            <Route path="/connections" element={<Navigate to="/proxies?tab=connections" replace />} />
            <Route path="/analytics" element={<Analytics />} />
            <Route path="/settings" element={<Settings />} />
          </Routes>
        </main>
      </div>
      <BottomNav />
      <ToastViewport />
    </div>
  );
}
