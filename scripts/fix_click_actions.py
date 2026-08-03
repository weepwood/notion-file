from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if old not in text:
        raise RuntimeError(f"pattern not found in {path}: {old[:120]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once(
    "src/App.tsx",
    'import { open, save } from "@tauri-apps/plugin-dialog";\n',
    'import { open, save } from "@tauri-apps/plugin-dialog";\nimport { openUrl } from "@tauri-apps/plugin-opener";\n',
)

replace_once(
    "src/App.tsx",
    "function localName(path: string): string {\n  return path.split(/[\\\\/]/).filter(Boolean).at(-1) || path;\n}\n",
    "function localName(path: string): string {\n  return path.split(/[\\\\/]/).filter(Boolean).at(-1) || path;\n}\n\nfunction normalizeExternalUrl(value: string): string {\n  const url = new URL(value);\n  if (url.protocol !== \"https:\" && url.protocol !== \"http:\") {\n    throw new Error(`不支持打开 ${url.protocol} 链接`);\n  }\n  return url.toString();\n}\n",
)

replace_once(
    "src/App.tsx",
    "  async function renameSelected() {\n",
    "  async function openSelectedNotionPage() {\n    if (!selected?.notionPageUrl) return;\n    try {\n      await openUrl(normalizeExternalUrl(selected.notionPageUrl));\n    } catch (error) {\n      setNotice({ type: \"error\", text: `无法使用系统浏览器打开 Notion 页面：${String(error)}` });\n    }\n  }\n\n  async function renameSelected() {\n",
)

replace_once(
    "src/App.tsx",
    'onClick={() => window.open(selected.notionPageUrl, "_blank")}><ExternalLink size={15} />Notion 页面</button>',
    'onClick={openSelectedNotionPage}><ExternalLink size={15} />Notion 页面</button>',
)

replace_once(
    "src-tauri/src/lib.rs",
    "    tauri::Builder::default()\n        .plugin(tauri_plugin_dialog::init())\n",
    "    tauri::Builder::default()\n        .plugin(tauri_plugin_dialog::init())\n        .plugin(tauri_plugin_opener::init())\n",
)

capability = Path("src-tauri/capabilities/default.json")
capability.write_text(
    '''{\n  "$schema": "../gen/schemas/desktop-schema.json",\n  "identifier": "default",\n  "description": "Default capability for the main window",\n  "windows": ["main"],\n  "permissions": [\n    "core:default",\n    "dialog:allow-open",\n    "dialog:allow-save",\n    "opener:default"\n  ]\n}\n''',
    encoding="utf-8",
)
