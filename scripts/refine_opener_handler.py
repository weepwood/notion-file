from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if old not in text:
        raise RuntimeError(f"pattern not found in {path}: {old[:160]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once(
    "src/App.tsx",
    'import { open, save } from "@tauri-apps/plugin-dialog";\n',
    'import { open, save } from "@tauri-apps/plugin-dialog";\nimport { openUrl } from "@tauri-apps/plugin-opener";\n',
)

replace_once(
    "src/App.tsx",
    "  async function renameSelected() {\n",
    '''  async function openSelectedNotionPage() {
    const url = selected?.notionPageUrl;
    if (!url) return;
    try {
      const parsed = new URL(url);
      if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
        throw new Error(`不支持打开 ${parsed.protocol} 链接`);
      }
      await openUrl(parsed.toString());
    } catch (error) {
      setNotice({
        type: "error",
        text: `无法使用系统浏览器打开 Notion 页面：${String(error)}`,
      });
    }
  }

  async function renameSelected() {
''',
)

replace_once(
    "src/App.tsx",
    'onClick={() => window.open(selected.notionPageUrl, "_blank")}><ExternalLink size={15} />Notion 页面</button>',
    'onClick={openSelectedNotionPage}><ExternalLink size={15} />Notion 页面</button>',
)

Path("src/main.tsx").write_text(
    '''import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";
import "./advanced.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
''',
    encoding="utf-8",
)
