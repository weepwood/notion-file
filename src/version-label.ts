import { getVersion } from "@tauri-apps/api/app";

const CLASSIC_VERSION_SELECTOR = ".drive-sidebar .brand span";

/**
 * 经典界面曾将版本号硬编码在 JSX 中，容易在发布时遗漏。
 * 这里统一读取 Tauri 应用版本，并在经典界面挂载时同步显示。
 */
export function installVersionLabelSync(): () => void {
  let disposed = false;
  const version = getVersion().catch(() => null);

  const sync = () => {
    void version.then((value) => {
      if (disposed || !value) return;
      const label = document.querySelector<HTMLElement>(CLASSIC_VERSION_SELECTOR);
      const expected = `个人云盘 · v${value}`;
      if (label && label.textContent !== expected) {
        label.textContent = expected;
      }
    });
  };

  sync();
  const observer = new MutationObserver(sync);
  observer.observe(document.body, { childList: true, subtree: true });

  return () => {
    disposed = true;
    observer.disconnect();
  };
}
