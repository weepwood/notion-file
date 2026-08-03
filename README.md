# Notion File

使用 Tauri 2、React 与 Rust 构建的本地文件上传与文件夹到 Notion 单向同步工具。

## 核心行为

### 单文件上传

1. 保存 Notion Token，并按需填写父页面链接或 ID。
2. 选择一个本地文件。
3. 选择在 Notion 中的显示方式：
   - **文件块（默认）**：显示为可下载附件。
   - **视频块**：仅用于视频，显示为可直接播放的视频。
4. 应用创建一篇与文件同名的 Notion 页面，并写入文件或视频块。
5. 不超过 1 MiB 的文本或代码文件会额外生成内容预览。
6. 每次成功或失败都会写入本地上传记录，包括文件路径、大小、SHA-256、时间、状态、Notion 页面地址、保存方式、分段数量和错误原因。
7. 不超过 20 MiB 的文件使用单段上传；更大的文件自动使用 10 MiB API 分片并在全部发送后完成合并。

### 超过 5 GiB 的视频

1. 应用自动检测本机的 `ffmpeg` 和 `ffprobe`。
2. 检测顺序包括 `FFMPEG_PATH`、程序目录、Windows WinGet 链接、常见安装目录和系统 `PATH`。
3. 视频超过 5 GiB 时，使用 ffmpeg 流复制切成约 4.8 GB 的可播放 MKV 分段。
4. 如果关键帧导致某段超过 4.8 GB，应用会自动缩短分段时长并重新切分，最多校准 5 次。
5. 每个视频分段再通过 Notion 的 10 MiB multi-part API 上传，并按顺序写入同一篇页面。
6. 临时视频分段保存在应用缓存目录，上传结束或失败后自动清理。
7. 非视频文件超过 5 GiB 时不会切分，因为 Notion 单个文件对象的上限仍是 5 GiB。

可通过环境变量指定 ffmpeg：

```text
FFMPEG_PATH=C:\ffmpeg\bin\ffmpeg.exe
```

也可以让 `ffmpeg` 和 `ffprobe` 位于系统 `PATH` 中。

### 文件夹同步

1. 选择一个本地文件夹。
2. 应用创建一篇与文件夹同名的 Notion 文档。
3. 文件夹中的文本、Markdown、图片、PDF 和其他附件统一整理到该文档中。
4. 通过 SHA-256 识别文件变化；没有变化时不会重复写入。
5. 需要更新时会先完整生成替换文档，确认写入成功后再将旧文档移入回收站，避免失败时清空原内容。

界面采用接近 Notion 的侧边栏、页面标题、内容块和中性色设计，但不复制 Notion 商标或专有视觉资产。

## Token 与父页面

- **个人访问令牌或 OAuth 公共连接**：通常只需 Token，应用可以尝试创建工作区级私有页面。
- **内部 Integration Token**：受 Notion API 限制，必须先在 Notion 中准备一个父页面，将 Integration 添加为该页面的连接，然后在应用的“内部 Integration 兼容设置”中填写父页面链接或 ID。

父页面 ID 因此不是所有用户的必填项，但内部 Integration 用户仍然需要配置。

## 文件大小与工作区限制

- 单文件上传支持 Notion 的 multi-part 流程：超过 20 MiB 时自动按 10 MiB 分片上传。
- Notion 付费工作区的 API bot 单个文件对象最多为 5 GiB。
- 超过 5 GiB 的视频会先由本地 ffmpeg 切成多个约 4.8 GB 的独立文件对象。
- Notion 免费工作区仍受 5 MiB 单文件限制，超过限制时 Notion API 会拒绝请求，应用会把原因写入失败记录。
- 文件夹同步中的附件目前仍沿用原有单段上传路径，单个附件上限为 20 MiB；后续会统一到同一套分片上传实现。
- 不超过 1 MiB 的文本文件会转换为 Notion 正文块；更大的文本文件作为附件上传。

## 其他限制

- ffmpeg 默认使用流复制，不重新编码视频，因此切分速度较快且不会主动降低画质；分段位置取决于源视频关键帧。
- 当前是本地到 Notion 的单向同步，不会从 Notion 反向修改本地文件。
- 更新文件夹文档时会根据当前文件夹内容重建文档内容，本地删除的文件会从最新文档中移除。
- 更新成功后文档页面 ID 和 URL 可能变化；旧版本文档会被移入 Notion 回收站。
- 上传记录只保存在本地应用数据目录，最多保留最近 500 条；清空记录不会删除 Notion 中的页面或文件。

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
