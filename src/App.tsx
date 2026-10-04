import { useEffect } from "react";
import { Routes, Route, Navigate } from "react-router-dom";
import Sidebar from "@/components/Sidebar";
import TitleBar from "@/components/TitleBar";
import BottomNav from "@/components/BottomNav";
import ToastViewport from "@/components/ui/Toast";
import Dashboard from "@/pages/Dashboard";
import Proxies from "@/pages/Proxies";
import Subscription from "@/pages/Subscription";
import Rules from "@/pages/Rules";
import Connections from "@/pages/Connections";
import Settings from "@/pages/Settings";
import { useProStore } from "@/stores/pro";

export default function App() {
  const initPro = useProStore((s) => s.init);

  // 启动时读取 Pro 解锁状态（proUnlocked 持久化于设置）
  useEffect(() => {
    initPro();
  }, [initPro]);

  return (
    <div className="flex flex-col h-screen bg-surface text-gray-200 overflow-hidden">
      <TitleBar />
      <div className="flex flex-1 overflow-hidden">
        <Sidebar />
        <main className="flex-1 overflow-y-auto p-6 pb-16 md:pb-6">
          <Routes>
            <Route path="/" element={<Navigate to="/dashboard" replace />} />
            <Route path="/dashboard" element={<Dashboard />} />
            <Route path="/proxies" element={<Proxies />} />
            <Route path="/subscription" element={<Subscription />} />
            <Route path="/rules" element={<Rules />} />
            <Route path="/connections" element={<Connections />} />
            <Route path="/settings" element={<Settings />} />
          </Routes>
        </main>
      </div>
      <BottomNav />
      <ToastViewport />
    </div>
  );
}
