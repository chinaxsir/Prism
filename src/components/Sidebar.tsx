import { NavLink } from "react-router-dom";
import {
  LayoutDashboard,
  Globe,
  ListFilter,
  Activity,
  Settings as SettingsIcon,
} from "lucide-react";
import clsx from "clsx";

const navItems = [
  { to: "/dashboard", icon: LayoutDashboard, label: "仪表盘" },
  { to: "/proxies", icon: Globe, label: "节点" },
  { to: "/rules", icon: ListFilter, label: "规则" },
  { to: "/connections", icon: Activity, label: "连接" },
  { to: "/settings", icon: SettingsIcon, label: "设置" },
];

export default function Sidebar() {
  return (
    <aside className="w-56 bg-surface-card border-r border-white/5 flex flex-col p-4 gap-1">
      <div className="text-lg font-bold text-accent mb-6 px-2">Prism</div>
      {navItems.map(({ to, icon: Icon, label }) => (
        <NavLink
          key={to}
          to={to}
          className={({ isActive }) =>
            clsx(
              "flex items-center gap-3 px-3 py-2 rounded-lg transition-colors",
              isActive
                ? "bg-accent/20 text-accent"
                : "text-gray-400 hover:bg-surface-hover hover:text-gray-200"
            )
          }
        >
          <Icon size={18} />
          <span>{label}</span>
        </NavLink>
      ))}
    </aside>
  );
}
