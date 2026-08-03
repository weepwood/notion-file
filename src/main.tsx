import React from "react";
import ReactDOM from "react-dom/client";
import { openUrl } from "@tauri-apps/plugin-opener";
import App from "./App";
import "./styles.css";
import "./advanced.css";

const browserWindowOpen = window.open.bind(window);

window.open = ((url?: string | URL, target?: string, features?: string) => {
  const value = typeof url === "string" ? url : url?.toString();
  if (value) {
    try {
      const parsed = new URL(value);
      if (parsed.protocol === "https:" || parsed.protocol === "http:") {
        void openUrl(parsed.toString()).catch((error) => {
          window.alert(`无法使用系统浏览器打开链接：${String(error)}`);
        });
        return null;
      }
    } catch {
      // 非 URL 内容继续交给 WebView 原生 window.open 处理。
    }
  }
  return browserWindowOpen(value, target, features);
}) as typeof window.open;

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
