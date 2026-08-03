# Notion File

使用 **Tauri 2 + React + Rust** 构建的 Notion 个人云盘管理器。

Notion File 将真实文件存放在 Notion 中，并通过本地桌面客户端提供虚拟目录、上传、下载、搜索、移动、重命名、回收站、断点续传、文件夹批量下载和文件版本管理。原有单文件上传和文件夹转文档功能继续保留。

## v0.6.0：续传、批量下载与版本历史

### HTTP Range 断点续传

下载过程写入 `<目标文件>.part`：

1. 新下载任务先检查现有 `.part` 文件长度。
2. 已存在临时文件时发送 `Range: bytes=<offset>-`。
3. 服务端返回 `206 Partial Content` 时继续追加写入。
4. 服务端忽略 Range 并返回 `200` 时，安全清空临时文件后重新下载。
5. 下载地址失效时重新读取 Notion 文件块，刷新签名地址。
6. 完成后执行 SHA-256 校验，再改名为正式文件。

下载进度每累计约 8 MiB 写回 SQLite。失败任务会保留临时文件，并可在“传输中心”点击“续传”。

### 文件夹批量下载

文件夹详情提供“下载文件夹”操作：

- 递归读取虚拟目录下的全部有效文件。
- 在用户选择的本地目录中创建同名根文件夹。
- 保持子目录结构逐项下载。
- 每个文件独立建立下载记录、支持续传和 SHA-256 校验。
- 批量结束后汇总成功与失败数量。

### 文件版本历史

文件详情提供“上传新版本”：

- 上传前保存当前版本记录。
- 新版本复用既有 single-part / multi-part 上传流程。
- 相同 SHA-256 内容优先复用已有 File Upload ID。
- 新附件作为新的文件块追加到原 Notion 页面。
- 当前节点的 Block ID、大小、MIME、SHA-256 和版本号更新为最新版本。
- 旧文件块不会删除，可在版本列表中独立下载并校验。

本地版本索引保存在 `drive_versions` 表中。当前版本附件始终保存在 Notion；历史版本的跨设备完整索引重建仍属于后续阶段。

## 云盘初始化

### 新建云盘

1. 在 Notion 中创建一个父页面。
2. 将 Internal Integration 添加到该页面的 Connections。
3. 在应用“连接设置”中保存 Token 和父页面链接。
4. 点击“初始化或连接云盘”。
5. 应用会创建 `Notion Drive` Database 和初始 Data Source。

### 连接已有云盘

填写已有 Database ID 和 Data Source ID 后，可以不填写父页面。应用会直接验证 Data Source，并从远端索引重建本地 SQLite。

## 远端数据结构

每个文件或文件夹对应 Data Source 中的一条页面记录，包含：

- Node ID 与 Parent ID
- Node Type 与逻辑路径
- MIME、大小和 SHA-256
- File Upload ID 与当前文件 Block ID
- 状态、版本和修改时间

文件页面内部保存真实 Notion 文件块；文件夹页面只保存虚拟目录元数据。

## 上传

- 支持多文件顺序上传。
- 20 MiB 以内使用 single-part。
- 超过 20 MiB 自动按 10 MiB multi-part 分片上传。
- 单个 Notion 文件对象上限为 5 GiB。
- 上传前计算 SHA-256；相同内容复用已存在的 File Upload ID。
- 上传和版本更新均写入 SQLite 传输记录。

## 文件管理

当前支持：

- 虚拟文件夹树与面包屑导航
- 搜索名称和逻辑路径
- 新建文件夹
- 多文件上传
- 单文件下载与 Range 续传
- 文件夹递归下载
- 文件版本上传、列表和历史下载
- 重命名与移动
- 软删除与恢复
- 打开对应 Notion 页面
- 从 Notion 重建本地节点索引
- 清理已结束传输记录

移动和重命名只更新远端索引，不重新上传文件内容。

## SQLite

应用数据保存在 `notion-file.sqlite3`：

- `upload_history`：传统单文件上传历史
- `drive_nodes`：云盘本地镜像索引
- `drive_transfers`：上传、下载和续传记录
- `drive_versions`：本地文件版本索引
- `app_meta`：迁移状态

数据库启用 WAL 和 busy timeout。应用异常退出后，仍处于 queued/running 的任务会在下一次读取时标记为失败，下载任务可利用保留的 `.part` 文件继续。

## 传统功能

“传统同步”页面继续提供：

- 单文件上传
- 文件块或视频块
- 大文件分片
- 超过 5 GiB 视频 ffmpeg 切分
- 本地文件夹转 Notion 文档
- 文件变化检测

## Notion API 行为

- API 版本：`2026-03-11`。
- 所有请求共用全局限流器，启动间隔为 350 ms。
- HTTP 429 按 `Retry-After` 进入全局冷却。
- 幂等读取请求支持有限指数退避。
- 写请求不会在响应不明确时盲目重试。
- HTTP User-Agent 根据 Cargo 当前包版本动态生成。
- 签名文件 URL 不会作为永久数据保存。

## 当前限制

- 版本记录表目前是本地索引；跨设备只能自动恢复当前版本，完整历史索引重建尚未实现。
- 文件夹批量下载当前顺序执行，尚未提供并发数配置、暂停或整批取消。
- 上传任务尚未支持进程级暂停与恢复。
- 回收站为应用级软删除，不会立即物理清除 Notion 底层附件。
- 免费工作区的文件大小限制由 Notion 服务端决定。

## 开发

```bash
npm install
npm run tauri dev
```

检查前端：

```bash
npm run build
```

检查 Rust：

```bash
cd src-tauri
cargo check
cargo test --lib
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

Release 工作流仅发布 Windows x64 NSIS 安装包。
