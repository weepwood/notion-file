# Notion Backup

使用 **Tauri 2 + React + Rust** 构建的本地文件到 Notion 增量备份程序。

## 备份模型

- 一个本地文件对应一个 Notion 子页面。
- 文件首次出现时创建页面并上传原始文件。
- 文件内容变化时，在原页面追加一个新版本，不覆盖旧附件。
- 本地删除文件时，只在 Notion 页面追加“已删除”记录，远端历史版本仍然保留。
- 每次有变化的备份运行都会创建一份“备份记录”页面，保存统计信息和 JSON 恢复清单。
- SHA-256 用于判断文件是否变化，未变化文件不会重复上传。
- Token 保存在 Windows Credential Manager、macOS Keychain 或 Linux Secret Service 中。

## 已实现功能

- 多个独立备份任务
- 手动备份和应用运行期间的定时备份
- 递归扫描与增量上传
- Notion `single_part` 上传
- 大于 20 MiB 文件的 `multi_part` 分片上传
- 图片、PDF、音频、视频和普通附件块
- 小型文本文件的可搜索预览
- 非破坏式删除记录
- 最近 100 次本地备份历史
- 从 Notion 恢复每个文件的最新版本
- Windows NSIS 和 MSI 安装包

## Notion 配置

1. 在 Notion 集成设置中创建 Internal Integration。
2. 复制 `ntn_...` Token。
3. 打开作为备份根目录的 Notion 页面，通过页面菜单将该页面共享给集成。
4. 在应用中填写 Token、页面 ID 和本地目录。
5. 测试连接后运行首次备份。

页面 ID 可以使用带连字符或不带连字符的 32 位 UUID，也可以从页面 URL 中提取。应用使用 Notion API `2026-03-11`。

## 本地开发

```bash
npm install
npm run tauri dev
```

## Windows 打包

```powershell
npm install
npm run tauri build
```

构建结果位于：

- `src-tauri/target/release/bundle/nsis/`
- `src-tauri/target/release/bundle/msi/`

## 自动发布

提交信息以 `发布 v` 开头并推送到 `main` 后，GitHub Actions 会在 Windows Runner 中构建 NSIS 和 MSI，并创建对应 GitHub Release。

## 安全与限制

- 这是单向备份，不会删除 Notion 中已经上传的历史版本。
- 自动备份仅在应用运行期间执行；关闭应用后不会驻留后台。
- 恢复功能依赖本机保存的备份索引；Notion 中的备份记录页面同时保留 JSON 清单，便于人工排查。
- Notion 工作区仍可能限制单文件大小，最终上限由工作区套餐决定。
- 首次备份大量文件时会受到 Notion API 速率限制，建议按任务分批执行。

## License

MIT
