use anyhow::{Context, Result};
use reqwest::{multipart, Client, Response, StatusCode};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncReadExt;

const NOTION_BASE_URL: &str = "https://api.notion.com/v1";
const NOTION_VERSION: &str = "2026-03-11";
const SINGLE_PART_LIMIT: u64 = 20 * 1024 * 1024;
const MULTI_PART_CHUNK_SIZE: usize = 10 * 1024 * 1024;

#[derive(Clone)]
pub struct NotionClient {
    client: Client,
    token: String,
}

impl NotionClient {
    pub fn new(token: String) -> Result<Self> {
        let client = Client::builder()
            .user_agent("notion-backup/0.2.0")
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(300))
            .build()
            .context("无法初始化 HTTP 客户端")?;
        Ok(Self { client, token })
    }

    fn request(&self, method: reqwest::Method, url: String) -> reqwest::RequestBuilder {
        self.client.request(method, url)
            .bearer_auth(&self.token)
            .header("Notion-Version", NOTION_VERSION)
            .header("Accept", "application/json")
    }

    async fn parse(response: Response) -> Result<Value> {
        let status = response.status();
        let retry_after = response.headers().get("retry-after").and_then(|value| value.to_str().ok()).map(str::to_owned);
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&text).ok()
                .and_then(|value| value.get("message").and_then(Value::as_str).map(str::to_owned))
                .unwrap_or(text);
            if status == StatusCode::TOO_MANY_REQUESTS {
                anyhow::bail!("Notion API 请求过快，请稍后重试（Retry-After: {}）：{}", retry_after.unwrap_or_else(|| "未知".into()), message);
            }
            anyhow::bail!("Notion API 请求失败（{}）：{}", status.as_u16(), message);
        }
        if text.trim().is_empty() { return Ok(Value::Null); }
        serde_json::from_str(&text).context("Notion API 返回了无效 JSON")
    }

    pub async fn get_page_title(&self, page_id: &str) -> Result<String> {
        let page_id = normalize_page_id(page_id)?;
        let response = self.request(reqwest::Method::GET, format!("{NOTION_BASE_URL}/pages/{page_id}")).send().await?;
        let value = Self::parse(response).await?;
        let title = value.get("properties").and_then(Value::as_object).and_then(|properties| {
            properties.values().find_map(|property| property.get("title").and_then(Value::as_array).and_then(|items| items.first()).and_then(|item| item.get("plain_text")).and_then(Value::as_str))
        }).unwrap_or("目标页面");
        Ok(title.to_string())
    }

    pub async fn create_child_page(&self, root_page_id: &str, title: &str) -> Result<String> {
        let root_page_id = normalize_page_id(root_page_id)?;
        let body = json!({
            "parent": { "type": "page_id", "page_id": root_page_id },
            "properties": { "title": { "type": "title", "title": [{ "type": "text", "text": { "content": truncate_chars(title, 2000) } }] } }
        });
        let response = self.request(reqwest::Method::POST, format!("{NOTION_BASE_URL}/pages")).json(&body).send().await?;
        let value = Self::parse(response).await?;
        value.get("id").and_then(Value::as_str).map(str::to_owned).context("Notion 未返回新页面 ID")
    }

    pub async fn append_blocks(&self, page_id: &str, blocks: Vec<Value>) -> Result<()> {
        let page_id = normalize_page_id(page_id)?;
        for chunk in blocks.chunks(100) {
            let response = self.request(reqwest::Method::PATCH, format!("{NOTION_BASE_URL}/blocks/{page_id}/children"))
                .json(&json!({ "children": chunk }))
                .send().await?;
            Self::parse(response).await?;
            tokio::time::sleep(Duration::from_millis(350)).await;
        }
        Ok(())
    }

    pub async fn upload_file(&self, path: &Path, mime_type: &str) -> Result<String> {
        let metadata = tokio::fs::metadata(path).await?;
        let file_name = path.file_name().and_then(|value| value.to_str()).unwrap_or("file");
        if metadata.len() <= SINGLE_PART_LIMIT {
            self.upload_single_part(path, file_name, mime_type).await
        } else {
            self.upload_multi_part(path, file_name, mime_type, metadata.len()).await
        }
    }

    async fn upload_single_part(&self, path: &Path, file_name: &str, mime_type: &str) -> Result<String> {
        let created = Self::parse(self.request(reqwest::Method::POST, format!("{NOTION_BASE_URL}/file_uploads"))
            .json(&json!({ "mode": "single_part", "filename": file_name, "content_type": mime_type }))
            .send().await?).await?;
        let upload_id = created.get("id").and_then(Value::as_str).context("缺少 file_upload id")?.to_string();
        tokio::time::sleep(Duration::from_millis(350)).await;
        let upload_url = created.get("upload_url").and_then(Value::as_str).map(str::to_owned)
            .unwrap_or_else(|| format!("{NOTION_BASE_URL}/file_uploads/{upload_id}/send"));
        let bytes = tokio::fs::read(path).await?;
        let part = multipart::Part::bytes(bytes).file_name(file_name.to_string()).mime_str(mime_type)?;
        let response = self.client.post(upload_url)
            .bearer_auth(&self.token)
            .header("Notion-Version", NOTION_VERSION)
            .multipart(multipart::Form::new().part("file", part))
            .send().await?;
        let uploaded = Self::parse(response).await?;
        ensure_uploaded(&uploaded)?;
        tokio::time::sleep(Duration::from_millis(350)).await;
        Ok(upload_id)
    }

    async fn upload_multi_part(&self, path: &Path, file_name: &str, mime_type: &str, file_size: u64) -> Result<String> {
        let number_of_parts = file_size.div_ceil(MULTI_PART_CHUNK_SIZE as u64);
        let created = Self::parse(self.request(reqwest::Method::POST, format!("{NOTION_BASE_URL}/file_uploads"))
            .json(&json!({
                "mode": "multi_part",
                "number_of_parts": number_of_parts,
                "filename": file_name,
                "content_type": mime_type
            }))
            .send().await?).await?;
        let upload_id = created.get("id").and_then(Value::as_str).context("缺少 file_upload id")?.to_string();
        tokio::time::sleep(Duration::from_millis(350)).await;
        let upload_url = created.get("upload_url").and_then(Value::as_str).map(str::to_owned)
            .unwrap_or_else(|| format!("{NOTION_BASE_URL}/file_uploads/{upload_id}/send"));

        let mut file = tokio::fs::File::open(path).await?;
        for part_number in 1..=number_of_parts {
            let remaining = file_size.saturating_sub((part_number - 1) * MULTI_PART_CHUNK_SIZE as u64);
            let expected = remaining.min(MULTI_PART_CHUNK_SIZE as u64) as usize;
            let mut buffer = vec![0_u8; expected];
            file.read_exact(&mut buffer).await?;
            let part = multipart::Part::bytes(buffer)
                .file_name(file_name.to_string())
                .mime_str(mime_type)?;
            let form = multipart::Form::new()
                .text("part_number", part_number.to_string())
                .part("file", part);
            let response = self.client.post(&upload_url)
                .bearer_auth(&self.token)
                .header("Notion-Version", NOTION_VERSION)
                .multipart(form)
                .send().await?;
            Self::parse(response).await?;
            tokio::time::sleep(Duration::from_millis(350)).await;
        }

        let completed = Self::parse(self.request(reqwest::Method::POST, format!("{NOTION_BASE_URL}/file_uploads/{upload_id}/complete"))
            .json(&json!({}))
            .send().await?).await?;
        ensure_uploaded(&completed)?;
        Ok(upload_id)
    }

    pub async fn latest_file_url(&self, page_id: &str) -> Result<String> {
        let page_id = normalize_page_id(page_id)?;
        let mut cursor: Option<String> = None;
        let mut latest_url: Option<String> = None;
        loop {
            let mut url = format!("{NOTION_BASE_URL}/blocks/{page_id}/children?page_size=100");
            if let Some(value) = cursor.as_ref() {
                url.push_str("&start_cursor=");
                url.push_str(value);
            }
            let value = Self::parse(self.request(reqwest::Method::GET, url).send().await?).await?;
            if let Some(results) = value.get("results").and_then(Value::as_array) {
                for block in results {
                    let Some(kind) = block.get("type").and_then(Value::as_str) else { continue; };
                    if !matches!(kind, "file" | "image" | "pdf" | "audio" | "video") { continue; }
                    let Some(body) = block.get(kind) else { continue; };
                    if let Some(url) = body.get("file").and_then(|file| file.get("url")).and_then(Value::as_str) {
                        latest_url = Some(url.to_string());
                    }
                    if let Some(url) = body.get("external").and_then(|file| file.get("url")).and_then(Value::as_str) {
                        latest_url = Some(url.to_string());
                    }
                }
            }
            if !value.get("has_more").and_then(Value::as_bool).unwrap_or(false) { break; }
            cursor = value.get("next_cursor").and_then(Value::as_str).map(str::to_owned);
            if cursor.is_none() { break; }
        }
        latest_url.context("页面中没有可恢复的文件附件")
    }

    pub async fn download_file(&self, url: &str, destination: &Path) -> Result<()> {
        let response = self.client.get(url).send().await?;
        let status = response.status();
        if !status.is_success() { anyhow::bail!("下载备份文件失败（HTTP {}）", status.as_u16()); }
        if let Some(parent) = destination.parent() { tokio::fs::create_dir_all(parent).await?; }
        let bytes = response.bytes().await?;
        tokio::fs::write(destination, bytes).await?;
        Ok(())
    }
}

fn ensure_uploaded(value: &Value) -> Result<()> {
    match value.get("status").and_then(Value::as_str) {
        Some("uploaded") => Ok(()),
        Some(status) => anyhow::bail!("文件上传状态异常：{status}"),
        None => Ok(()),
    }
}

pub fn normalize_page_id(value: &str) -> Result<String> {
    let without_query = value.trim().split(|character| character == '?' || character == '#').next().unwrap_or(value.trim());
    let tail = without_query.rsplit('/').next().unwrap_or(without_query);
    let compact: String = tail.chars().filter(|character| character.is_ascii_hexdigit()).collect();
    if compact.len() < 32 { anyhow::bail!("页面 ID 格式无效，应包含 32 位十六进制 ID"); }
    let compact = &compact[compact.len() - 32..];
    Ok(format!("{}-{}-{}-{}-{}", &compact[0..8], &compact[8..12], &compact[12..16], &compact[16..20], &compact[20..32]))
}

pub fn file_block(upload_id: &str, mime_type: &str) -> Value {
    let block_type = if mime_type.starts_with("image/") { "image" }
        else if mime_type == "application/pdf" { "pdf" }
        else if mime_type.starts_with("audio/") { "audio" }
        else if mime_type.starts_with("video/") { "video" }
        else { "file" };
    let mut block = json!({ "object": "block", "type": block_type });
    block[block_type] = json!({ "type": "file_upload", "file_upload": { "id": upload_id } });
    block
}

pub fn version_heading(version: u32, timestamp: &str) -> Value {
    rich_text_block("heading_2", &format!("版本 {version} · {timestamp}"), None)
}

pub fn metadata_callout(relative_path: &str, hash: &str, size: u64, modified_at: i64) -> Value {
    rich_text_block("callout", &format!("路径：{relative_path}\nSHA-256：{hash}\n大小：{size} bytes\n本地修改时间戳：{modified_at}"), Some("📦"))
}

pub fn deletion_callout(relative_path: &str, timestamp: &str) -> Value {
    rich_text_block("callout", &format!("本地文件已删除，但远端历史版本保留。\n路径：{relative_path}\n记录时间：{timestamp}"), Some("🗃️"))
}

pub fn summary_blocks(summary: &str, manifest: &str) -> Vec<Value> {
    let mut blocks = vec![rich_text_block("callout", summary, Some("✅")), rich_text_block("heading_2", "恢复清单", None)];
    blocks.extend(code_block_values(manifest, "json"));
    blocks
}

pub fn preview_blocks(content: &str, extension: &str) -> Vec<Value> {
    let mut blocks = vec![rich_text_block("heading_3", "文本预览", None)];
    if matches!(extension, "md" | "markdown" | "mdx") {
        blocks.extend(markdown_blocks(content));
    } else {
        blocks.extend(code_block_values(content, notion_code_language(extension)));
    }
    blocks
}

fn markdown_blocks(content: &str) -> Vec<Value> {
    let mut blocks = Vec::new();
    for line in content.lines().take(500) {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        let (kind, text) = if let Some(value) = trimmed.strip_prefix("### ") { ("heading_3", value) }
            else if let Some(value) = trimmed.strip_prefix("## ") { ("heading_2", value) }
            else if let Some(value) = trimmed.strip_prefix("# ") { ("heading_1", value) }
            else if let Some(value) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) { ("bulleted_list_item", value) }
            else if let Some(value) = trimmed.strip_prefix("> ") { ("quote", value) }
            else { ("paragraph", trimmed) };
        blocks.extend(split_text(text).into_iter().map(|part| rich_text_block(kind, &part, None)));
    }
    if blocks.is_empty() { blocks.push(rich_text_block("paragraph", "（空文件）", None)); }
    blocks
}

fn code_block_values(content: &str, language: &str) -> Vec<Value> {
    let parts = split_text(content);
    if parts.is_empty() { return vec![rich_text_block("paragraph", "（空文件）", None)]; }
    parts.into_iter().map(|part| json!({
        "object": "block",
        "type": "code",
        "code": { "rich_text": [{ "type": "text", "text": { "content": part } }], "language": language, "caption": [] }
    })).collect()
}

fn rich_text_block(kind: &str, content: &str, emoji: Option<&str>) -> Value {
    let mut body = json!({ "rich_text": [{ "type": "text", "text": { "content": truncate_chars(content, 2000) } }] });
    if kind == "callout" { body["icon"] = json!({ "type": "emoji", "emoji": emoji.unwrap_or("📄") }); }
    let mut block = json!({ "object": "block", "type": kind });
    block[kind] = body;
    block
}

fn split_text(content: &str) -> Vec<String> {
    let chars: Vec<char> = content.chars().collect();
    chars.chunks(1900).map(|chunk| chunk.iter().collect()).collect()
}

fn truncate_chars(content: &str, maximum: usize) -> String { content.chars().take(maximum).collect() }

fn notion_code_language(extension: &str) -> &str {
    match extension.to_ascii_lowercase().as_str() {
        "rs" => "rust", "js" | "mjs" | "cjs" | "jsx" => "javascript", "ts" | "tsx" => "typescript", "py" => "python", "java" => "java", "go" => "go", "c" => "c", "cpp" | "cc" | "cxx" | "h" | "hpp" => "c++", "cs" => "c#", "sh" | "bash" => "shell", "ps1" => "powershell", "json" => "json", "yaml" | "yml" => "yaml", "toml" => "toml", "xml" => "xml", "html" => "html", "css" => "css", "sql" => "sql", "rb" => "ruby", "php" => "php", "swift" => "swift", "kt" | "kts" => "kotlin", _ => "plain text"
    }
}
