import { Routes, Route, Navigate } from "react-router-dom";
import Sidebar from "@/components/Sidebar";
import TitleBar from "@/components/TitleBar";
import Dashboard from "@/pages/Dashboard";
import Proxies from "@/pages/Proxies";
import Rules from "@/pages/Rules";
import Connections from "@/pages/Connections";
import Settings from "@/pages/Settings";

export default function App() {
  return (
    <div className="flex flex-col h-screen bg-surface text-gray-200 overflow-hidden">
      <TitleBar />
      <div className="flex flex-1 overflow-hidden">
        <Sidebar />
        <main className="flex-1 overflow-y-auto p-6">
          <Routes>
            <Route path="/" element={<Navigate to="/dashboard" replace />} />
            <Route path="/dashboard" element={<Dashboard />} />
            <Route path="/proxies" element={<Proxies />} />
            <Route path="/rules" element={<Rules />} />
            <Route path="/connections" element={<Connections />} />
            <Route path="/settings" element={<Settings />} />
          </Routes>
        </main>
      </div>
    </div>
  );
}
