# Notion File

使用 **Tauri 2 + React + Rust** 构建的 Notion 个人云盘管理器。

Notion File 将真实文件存放在 Notion 中，并通过本地桌面客户端提供虚拟目录、上传、下载、搜索、移动、重命名、回收站和完整性校验。原有单文件上传和文件夹转文档功能继续保留。

## v0.5.0：Notion Drive

### 云盘初始化

1. 在 Notion 中创建一个父页面。
2. 将你的 Internal Integration 添加到该页面的 Connections。
3. 在应用“连接设置”中保存 Token 和父页面链接。
4. 点击“初始化或连接云盘”。
5. 应用会在父页面下创建一个 `Notion Drive` Database，并创建初始 Data Source。

跨设备使用时，可以填写已有的 Database ID 和 Data Source ID，再执行连接。应用会从远端索引重建本地 SQLite。

### 远端数据结构

每个文件或文件夹对应 Data Source 中的一条页面记录，包含：

- Node ID
- Parent ID
- Node Type
- 逻辑路径
- MIME、大小和 SHA-256
- File Upload ID
- 文件 Block ID
- 状态、版本和修改时间

文件页面内部附加真实 Notion 文件块；文件夹只保存虚拟目录元数据。

### 上传

- 支持一次选择多个文件并顺序上传。
- 20 MiB 以内使用 single-part。
- 超过 20 MiB 自动按 10 MiB multi-part 分片上传。
- 单个 Notion 文件对象上限为 5 GiB。
- 上传前计算 SHA-256；发现相同内容时复用已存在的 File Upload ID，避免重复传输文件内容。
- 上传任务和结果写入 SQLite 传输记录。

### 下载

1. 根据本地索引取得 Notion page ID 与 block ID。
2. 调用 Notion API 获取当前有效的签名下载地址。
3. 流式写入 `.part` 临时文件。
4. 下载后重新计算 SHA-256。
5. 校验成功后替换为正式文件名。

Notion 返回的文件 URL 是短期签名地址，程序不会把 URL 当作永久数据保存。

### 文件管理

当前基础版支持：

- 虚拟文件夹树
- 搜索名称和逻辑路径
- 新建文件夹
- 文件上传和下载
- 重命名
- 移动文件或文件夹
- 软删除与恢复
- 打开对应 Notion 页面
- 从 Notion 重建本地索引
- 清理已完成或失败的传输记录

移动和重命名只修改远端索引，不会重新上传文件内容。

## SQLite

应用数据保存在 `notion-file.sqlite3`：

- `upload_history`：旧版单文件上传历史
- `drive_nodes`：云盘本地镜像索引
- `drive_transfers`：上传和下载记录
- `app_meta`：迁移状态

数据库启用 WAL、外键和 busy timeout。应用异常退出后，仍处于 queued/running 的传输会在下一次读取时标记为失败。

## 原有功能

“传统同步”页面继续提供：

- 单文件上传
- 文件块或视频块
- 大文件分片
- 超过 5 GB 视频 ffmpeg 切分
- 本地文件夹转 Notion 文档
- 文件变化检测

## Notion API 行为

- 所有 API 请求共用全局限流器，启动间隔为 350 ms。
- HTTP 429 按 `Retry-After` 进入全局冷却。
- 幂等读取请求支持有限指数退避。
- 写请求不会在响应不明确时盲目重试，避免重复创建页面或区块。
- 使用 Notion API `2026-03-11`。

## 当前限制

- v0.5.0 下载失败后会保留 `.part` 文件，但尚未提供 HTTP Range 断点续传。
- 文件夹批量下载和版本历史计划在后续版本实现。
- 回收站为应用级软删除；不会立即物理删除 Notion 底层文件。
- 不建议手动修改 `Notion Drive` 数据库的系统属性。
- 免费工作区的文件大小限制仍由 Notion 服务端决定。

## 开发

```bash
npm install
npm run tauri dev
```

检查前端：

```bash
npm run build
```

构建 Windows NSIS：

```bash
npm run desktop:build
```

## 发布

同步更新以下版本号并合并到 `main`：

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

Release 工作流只构建并保留：

```text
Notion.File_<version>_x64-setup.exe
```
