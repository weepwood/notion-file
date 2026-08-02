use anyhow::{Context, Result};
use reqwest::{multipart, Client, Response};
use serde_json::{json, Value};
use std::path::Path;

const NOTION_BASE_URL: &str = "https://api.notion.com/v1";
const NOTION_VERSION: &str = "2026-03-11";
const MAX_SINGLE_PART_FILE_SIZE: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct CreatedPage {
    pub id: String,
    pub url: Option<String>,
}

#[derive(Clone)]
pub struct NotionClient {
    client: Client,
    token: String,
}

impl NotionClient {
    pub fn new(token: String) -> Result<Self> {
        let client = Client::builder()
            .user_agent("notion-file/0.2.0")
            .build()
            .context("无法初始化 HTTP 客户端")?;
        Ok(Self { client, token })
    }

    fn request(&self, method: reqwest::Method, url: String) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .bearer_auth(&self.token)
            .header("Notion-Version", NOTION_VERSION)
            .header("Accept", "application/json")
    }

    async fn parse(response: Response) -> Result<Value> {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|value| {
                    value
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or(text);
            anyhow::bail!("Notion API 请求失败（{}）：{}", status.as_u16(), message);
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).context("Notion API 返回了无效 JSON")
    }

    pub async fn get_connection_label(&self) -> Result<String> {
        let response = self
            .request(reqwest::Method::GET, format!("{NOTION_BASE_URL}/users/me"))
            .send()
            .await?;
        let value = Self::parse(response).await?;
        let label = value
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/bot/owner/user/name").and_then(Value::as_str))
            .unwrap_or("Notion 连接可用");
        Ok(label.to_string())
    }

    pub async fn get_page_title(&self, page_id: &str) -> Result<String> {
        let page_id = normalize_page_id(page_id)?;
        let response = self
            .request(
                reqwest::Method::GET,
                format!("{NOTION_BASE_URL}/pages/{page_id}"),
            )
            .send()
            .await?;
        let value = Self::parse(response).await?;
        let title = value
            .get("properties")
            .and_then(Value::as_object)
            .and_then(|properties| {
                properties.values().find_map(|property| {
                    property
                        .get("title")
                        .and_then(Value::as_array)
                        .and_then(|items| items.first())
                        .and_then(|item| item.get("plain_text"))
                        .and_then(Value::as_str)
                })
            })
            .unwrap_or("目标页面");
        Ok(title.to_string())
    }

    pub async fn create_document_page(
        &self,
        parent_page_id: Option<&str>,
        title: &str,
    ) -> Result<CreatedPage> {
        let mut body = json!({
            "properties": {
                "title": {
                    "type": "title",
                    "title": [{
                        "type": "text",
                        "text": { "content": truncate_chars(title, 2000) }
                    }]
                }
            },
            "icon": { "type": "emoji", "emoji": "📁" }
        });

        body["parent"] = match parent_page_id.filter(|value| !value.trim().is_empty()) {
            Some(page_id) => json!({
                "type": "page_id",
                "page_id": normalize_page_id(page_id)?
            }),
            None => json!({
                "type": "workspace",
                "workspace": true
            }),
        };

        let response = self
            .request(reqwest::Method::POST, format!("{NOTION_BASE_URL}/pages"))
            .json(&body)
            .send()
            .await?;
        let value = Self::parse(response).await?;
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .context("Notion 未返回新页面 ID")?;
        let url = value
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| Some(page_url_from_id(&id)));
        Ok(CreatedPage { id, url })
    }

    pub async fn archive_page(&self, page_id: &str) -> Result<()> {
        let page_id = normalize_page_id(page_id)?;
        let response = self
            .request(
                reqwest::Method::PATCH,
                format!("{NOTION_BASE_URL}/pages/{page_id}"),
            )
            .json(&json!({ "in_trash": true }))
            .send()
            .await?;
        Self::parse(response).await?;
        Ok(())
    }

    pub async fn clear_page(&self, page_id: &str) -> Result<()> {
        let page_id = normalize_page_id(page_id)?;
        let mut cursor: Option<String> = None;
        let mut block_ids = Vec::new();

        loop {
            let mut url = format!("{NOTION_BASE_URL}/blocks/{page_id}/children?page_size=100");
            if let Some(value) = cursor.as_ref() {
                url.push_str("&start_cursor=");
                url.push_str(value);
            }
            let value =
                Self::parse(self.request(reqwest::Method::GET, url).send().await?).await?;
            if let Some(results) = value.get("results").and_then(Value::as_array) {
                block_ids.extend(
                    results
                        .iter()
                        .filter_map(|block| block.get("id").and_then(Value::as_str))
                        .map(str::to_owned),
                );
            }
            if !value
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                break;
            }
            cursor = value
                .get("next_cursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }

        for block_id in block_ids {
            Self::parse(
                self.request(
                    reqwest::Method::DELETE,
                    format!("{NOTION_BASE_URL}/blocks/{block_id}"),
                )
                .send()
                .await?,
            )
            .await?;
        }
        Ok(())
    }

    pub async fn append_blocks(&self, page_id: &str, blocks: Vec<Value>) -> Result<()> {
        if blocks.is_empty() {
            return Ok(());
        }
        let page_id = normalize_page_id(page_id)?;
        for chunk in blocks.chunks(100) {
            let response = self
                .request(
                    reqwest::Method::PATCH,
                    format!("{NOTION_BASE_URL}/blocks/{page_id}/children"),
                )
                .json(&json!({ "children": chunk }))
                .send()
                .await?;
            Self::parse(response).await?;
        }
        Ok(())
    }

    pub async fn upload_file(&self, path: &Path, mime_type: &str) -> Result<String> {
        let metadata = tokio::fs::metadata(path).await?;
        if metadata.len() > MAX_SINGLE_PART_FILE_SIZE {
            anyhow::bail!("文件超过 20 MiB，当前版本暂不支持分片上传");
        }

        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("file");

        let created = Self::parse(
            self.request(
                reqwest::Method::POST,
                format!("{NOTION_BASE_URL}/file_uploads"),
            )
            .json(&json!({
                "mode": "single_part",
                "filename": file_name,
                "content_type": mime_type
            }))
            .send()
            .await?,
        )
        .await?;

        let upload_id = created
            .get("id")
            .and_then(Value::as_str)
            .context("缺少 file_upload id")?;
        let upload_url = created
            .get("upload_url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{NOTION_BASE_URL}/file_uploads/{upload_id}/send"));

        let bytes = tokio::fs::read(path).await?;
        let part = multipart::Part::bytes(bytes)
            .file_name(file_name.to_string())
            .mime_str(mime_type)?;
        let response = self
            .client
            .post(upload_url)
            .bearer_auth(&self.token)
            .header("Notion-Version", NOTION_VERSION)
            .multipart(multipart::Form::new().part("file", part))
            .send()
            .await?;
        let uploaded = Self::parse(response).await?;

        if uploaded.get("status").and_then(Value::as_str) != Some("uploaded") {
            anyhow::bail!("文件上传后未进入 uploaded 状态");
        }
        Ok(upload_id.to_string())
    }
}

pub fn normalize_page_id(value: &str) -> Result<String> {
    let without_query = value.trim().split('?').next().unwrap_or(value.trim());
    let tail = without_query
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(without_query);
    let mut compact: String = tail
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .collect();

    if compact.len() > 32 {
        compact = compact[compact.len() - 32..].to_string();
    }
    if compact.len() != 32 {
        anyhow::bail!("页面 ID 格式无效，应为 32 位十六进制 ID 或 Notion 页面链接");
    }

    Ok(format!(
        "{}-{}-{}-{}-{}",
        &compact[0..8],
        &compact[8..12],
        &compact[12..16],
        &compact[16..20],
        &compact[20..32]
    ))
}

pub fn page_url_from_id(page_id: &str) -> String {
    let compact: String = page_id
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .collect();
    format!("https://www.notion.so/{compact}")
}

pub fn text_blocks(content: &str, extension: &str) -> Vec<Value> {
    if matches!(extension, "md" | "markdown" | "mdx") {
        markdown_blocks(content)
    } else {
        code_blocks(content, notion_code_language(extension))
    }
}

pub fn file_block(upload_id: &str, mime_type: &str) -> Value {
    let block_type = if mime_type.starts_with("image/") {
        "image"
    } else if mime_type == "application/pdf" {
        "pdf"
    } else if mime_type.starts_with("audio/") {
        "audio"
    } else if mime_type.starts_with("video/") {
        "video"
    } else {
        "file"
    };
    let mut block = json!({ "object": "block", "type": block_type });
    block[block_type] = json!({
        "type": "file_upload",
        "file_upload": { "id": upload_id }
    });
    block
}

pub fn folder_summary_callout(
    folder_name: &str,
    file_count: usize,
    total_bytes: u64,
    synced_at: &str,
) -> Value {
    rich_text_block(
        "callout",
        &format!(
            "本地文件夹：{folder_name}\n文件数：{file_count}\n总大小：{}\n同步时间：{synced_at}",
            format_bytes(total_bytes)
        ),
        Some("🔄"),
    )
}

pub fn metadata_callout(relative_path: &str, mime_type: &str, size: u64, hash: &str) -> Value {
    rich_text_block(
        "callout",
        &format!(
            "路径：{relative_path}\n类型：{mime_type}\n大小：{}\nSHA-256：{hash}",
            format_bytes(size)
        ),
        Some("📄"),
    )
}

pub fn heading_block(content: &str) -> Value {
    rich_text_block("heading_2", content, None)
}

pub fn divider_block() -> Value {
    json!({ "object": "block", "type": "divider", "divider": {} })
}

fn markdown_blocks(content: &str) -> Vec<Value> {
    let mut blocks = Vec::new();
    let mut in_code = false;
    let mut language = String::from("plain text");
    let mut code = String::new();

    for line in content.lines() {
        if line.trim_start().starts_with("```") {
            if in_code {
                blocks.extend(code_block_values(&code, &language));
                code.clear();
                in_code = false;
            } else {
                language = line.trim().trim_start_matches("```").trim().to_string();
                if language.is_empty() {
                    language = "plain text".to_string();
                }
                in_code = true;
            }
            continue;
        }
        if in_code {
            code.push_str(line);
            code.push('\n');
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (kind, text) = if let Some(value) = trimmed.strip_prefix("### ") {
            ("heading_3", value)
        } else if let Some(value) = trimmed.strip_prefix("## ") {
            ("heading_2", value)
        } else if let Some(value) = trimmed.strip_prefix("# ") {
            ("heading_1", value)
        } else if let Some(value) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            ("bulleted_list_item", value)
        } else if let Some(value) = trimmed.strip_prefix("> ") {
            ("quote", value)
        } else {
            ("paragraph", trimmed)
        };
        blocks.extend(
            split_text(text)
                .into_iter()
                .map(|part| rich_text_block(kind, &part, None)),
        );
    }

    if in_code && !code.is_empty() {
        blocks.extend(code_block_values(&code, &language));
    }
    if blocks.is_empty() {
        blocks.push(rich_text_block("paragraph", "（空文件）", None));
    }
    blocks
}

fn code_blocks(content: &str, language: &str) -> Vec<Value> {
    let mut blocks = code_block_values(content, language);
    if blocks.is_empty() {
        blocks.push(rich_text_block("paragraph", "（空文件）", None));
    }
    blocks
}

fn code_block_values(content: &str, language: &str) -> Vec<Value> {
    split_text(content)
        .into_iter()
        .map(|part| {
            json!({
                "object": "block",
                "type": "code",
                "code": {
                    "rich_text": [{
                        "type": "text",
                        "text": { "content": part }
                    }],
                    "language": language,
                    "caption": []
                }
            })
        })
        .collect()
}

fn rich_text_block(kind: &str, content: &str, emoji: Option<&str>) -> Value {
    let mut body = json!({
        "rich_text": [{
            "type": "text",
            "text": { "content": truncate_chars(content, 2000) }
        }]
    });
    if kind == "callout" {
        body["icon"] = json!({
            "type": "emoji",
            "emoji": emoji.unwrap_or("📄")
        });
    }
    let mut block = json!({ "object": "block", "type": kind });
    block[kind] = body;
    block
}

fn split_text(content: &str) -> Vec<String> {
    let chars: Vec<char> = content.chars().collect();
    chars
        .chunks(1900)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

fn truncate_chars(content: &str, maximum: usize) -> String {
    content.chars().take(maximum).collect()
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut index = 0;
    while value >= 1024.0 && index < UNITS.len() - 1 {
        value /= 1024.0;
        index += 1;
    }
    if index == 0 {
        format!("{bytes} {}", UNITS[index])
    } else {
        format!("{value:.1} {}", UNITS[index])
    }
}

fn notion_code_language(extension: &str) -> &str {
    match extension.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "py" => "python",
        "java" => "java",
        "go" => "go",
        "c" => "c",
        "cpp" | "cc" | "cxx" | "h" | "hpp" => "c++",
        "cs" => "c#",
        "sh" | "bash" => "shell",
        "ps1" => "powershell",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "xml" => "xml",
        "html" => "html",
        "css" => "css",
        "sql" => "sql",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        _ => "plain text",
    }
}
