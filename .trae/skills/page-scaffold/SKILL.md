---
name: page-scaffold
description: Generate a complete Prism client page wired end to end, including React page, route, sidebar entry, Tauri command and API wrapper. Use when the user asks to add or create a new page/module/view. Do not use for editing an existing page.
---

# Page Scaffold (Prism)

新增一个完整页面时，严格按以下顺序落盘。所有代码遵循本仓库既有约定，不要发明新的目录或命名风格。

## 0. 命名

- 页面名：PascalCase，如 `LogArchive`；文件 `src/pages/LogArchive.tsx`
- 路由路径：kebab-case，如 `/log-archive`
- 前端 API 函数：camelCase，如 `getLogArchives`
- Rust 命令：snake_case，如 `get_log_archives`
- 页面上的可见文案使用中文。

## 1. 前端页面 `src/pages/{Name}.tsx`

参照既有页面结构，默认骨架：

```tsx
import { useEffect, useState } from "react";
import { getLogArchives } from "@/api/ipc";

export default function LogArchive() {
  const [data, setData] = useState([]);

  useEffect(() => {
    getLogArchives().then((d) => setData(d as never[])).catch(console.error);
  }, []);

  return (
    <div className="space-y-6">
      <h1 className="text-2xl font-bold">日志归档</h1>
      <section className="bg-surface-card rounded-xl p-5">{/* 内容 */}</section>
    </div>
  );
}
```

约定：
- 容器统一 `space-y-6`；卡片统一 `bg-surface-card rounded-xl p-5`
- 图标只用 `lucide-react`；条件类名用 `clsx`
- 实时数据用 `listen` 订阅 Tauri 事件，一次性数据用 `@/api/ipc` 的 invoke 封装
- 共享状态放进 `src/stores/core.ts` 的 zustand store；页面局部状态用 useState
- 不要引入新依赖，除非用户明确同意

## 2. 注册路由 `src/App.tsx`

- 顶部 `import LogArchive from "@/pages/LogArchive";`
- 在 `<Routes>` 内加 `<Route path="/log-archive" element={<LogArchive />} />`

## 3. 加入侧栏 `src/components/Sidebar.tsx`

在 `navItems` 数组追加一项（从 lucide-react 选图标）：

```tsx
{ to: "/log-archive", icon: Archive, label: "归档" },
```

## 4. Rust 命令 `src-tauri/src/ipc/commands.rs`

需要后端能力时才加。命令骨架：

```rust
#[tauri::command]
pub async fn get_log_archives() -> Result<serde_json::Value, String> {
    // 业务逻辑放 core/ 或 platform/，commands 只做参数与错误转换
    Ok(serde_json::json!([]))
}
```

错误统一 `.map_err(|e| e.to_string())`。请求参数用 derive Deserialize 的 DTO。

然后在 `src-tauri/src/lib.rs` 的 `generate_handler!` 列表中登记该命令。

## 5. 前端 API 封装 `src/api/ipc.ts`

```ts
export const getLogArchives = () => invoke("get_log_archives");
```

参数名与 Rust 命令形参一致（Rust snake_case 形参从前端 camelCase 传入时 Tauri 自动转换，按既有调用写法书写）。

## 6. 完成检查

- 路由可访问，侧栏高亮正确
- invoke 命令名在 Rust 与前端完全一致
- GetDiagnostics 无错误
- 无多余文件（不建 README/文档）
