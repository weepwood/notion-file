# Notion File

使用 Tauri 2、React 与 Rust 构建的本地文件夹到 Notion 单向同步工具。

## 核心行为

1. 保存 Notion Token。
2. 选择一个本地文件夹。
3. 应用创建一篇与文件夹同名的 Notion 文档。
4. 文件夹中的文本、Markdown、图片、PDF 和其他附件统一整理到该文档中。
5. 通过 SHA-256 识别文件变化；没有变化时不会重复写入。
6. 需要更新时会先完整生成替换文档，确认写入成功后再将旧文档移入回收站，避免失败时清空原内容。

界面采用接近 Notion 的侧边栏、页面标题、内容块和中性色设计，但不复制 Notion 商标或专有视觉资产。

## Token 与父页面

- **个人访问令牌或 OAuth 公共连接**：通常只需 Token 和本地文件夹，应用可以尝试创建工作区级私有页面。
- **内部 Integration Token**：受 Notion API 限制，必须先在 Notion 中准备一个父页面，将 Integration 添加为该页面的连接，然后在应用的“内部 Integration 兼容设置”中填写父页面链接或 ID。

父页面 ID 因此不是所有用户的必填项，但内部 Integration 用户仍然需要配置。

## 当前限制

- 单个非文本文件上传上限为 20 MiB。
- 不超过 1 MiB 的文本文件会转换为 Notion 正文块；更大的文本文件作为附件上传。
- 当前是本地到 Notion 的单向同步，不会从 Notion 反向修改本地文件。
- 更新文档时会根据当前文件夹内容重建文档内容，本地删除的文件会从最新文档中移除。
- 更新成功后文档页面 ID 和 URL 可能变化；旧版本文档会被移入 Notion 回收站。

## 开发

```bash
npm install
npm run tauri dev
```

构建 Windows 安装包：

```bash
npm run desktop:build
```

## 发布

同时更新 `package.json`、`src-tauri/Cargo.toml` 和 `src-tauri/tauri.conf.json` 中的版本号并合并到 `main` 后，GitHub Actions 会自动构建 Windows NSIS、MSI 和可执行文件，并创建对应的 GitHub Release。
