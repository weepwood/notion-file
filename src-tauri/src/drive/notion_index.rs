use crate::models::DriveNode;
use crate::notion::{file_block, normalize_page_id, page_url_from_id};
use crate::notion_request::{parse_json_response, NotionHttp};
use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::Method;
use serde_json::{json, Map, Value};

const NOTION_BASE_URL: &str = "https://api.notion.com/v1";

pub(super) async fn create_drive_database(
    http: &NotionHttp,
    root_page_id: &str,
) -> Result<(String, String)> {
    let root_page_id = normalize_page_id(root_page_id)?;
    let body = json!({
        "parent": { "type": "page_id", "page_id": root_page_id },
        "title": [{ "type": "text", "text": { "content": "Notion Drive" } }],
        "description": [{
            "type": "text",
            "text": { "content": "由 Notion File 管理的远端文件索引，请勿随意删除系统属性。" }
        }],
        "is_inline": false,
        "initial_data_source": {
            "title": [{ "type": "text", "text": { "content": "Files" } }],
            "properties": drive_schema()
        },
        "icon": { "type": "emoji", "emoji": "☁️" }
    });

    let value = parse_json_response(
        http.request(Method::POST, format!("{NOTION_BASE_URL}/databases"))
            .json(&body)
            .send()
            .await
            .context("创建 Notion Drive 数据库请求失败")?,
        "创建 Notion Drive 数据库",
    )
    .await?;

    let database_id = value
        .get("id")
        .and_then(Value::as_str)
        .context("Notion 未返回云盘 database_id")?
        .to_string();
    let data_source_id = value
        .get("data_sources")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            value
                .get("initial_data_source")
                .and_then(|source| source.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        });

    let data_source_id = match data_source_id {
        Some(value) => value,
        None => retrieve_first_data_source(http, &database_id).await?,
    };
    Ok((database_id, data_source_id))
}

pub(super) async fn verify_data_source(
    http: &NotionHttp,
    data_source_id: &str,
) -> Result<()> {
    parse_json_response(
        http.request(
            Method::GET,
            format!("{NOTION_BASE_URL}/data_sources/{data_source_id}"),
        )
        .send()
        .await
        .context("验证云盘数据源请求失败")?,
        "验证云盘数据源",
    )
    .await?;
    Ok(())
}

pub(super) async fn fetch_remote_nodes(
    http: &NotionHttp,
    data_source_id: &str,
) -> Result<Vec<DriveNode>> {
    let mut cursor: Option<String> = None;
    let mut nodes = Vec::new();

    loop {
        let mut body = json!({ "page_size": 100 });
        if let Some(value) = cursor.as_ref() {
            body["start_cursor"] = json!(value);
        }
        let value = parse_json_response(
            http.request(
                Method::POST,
                format!("{NOTION_BASE_URL}/data_sources/{data_source_id}/query"),
            )
            .json(&body)
            .send()
            .await
            .context("查询 Notion Drive 远端索引失败")?,
            "查询 Notion Drive 远端索引",
        )
        .await?;

        if let Some(results) = value.get("results").and_then(Value::as_array) {
            for page in results {
                if let Some(node) = remote_page_to_node(page)? {
                    nodes.push(node);
                }
            }
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

    Ok(nodes)
}

pub(super) async fn create_remote_node_page(
    http: &NotionHttp,
    data_source_id: &str,
    node: &DriveNode,
) -> Result<(String, Option<String>)> {
    let body = json!({
        "parent": { "type": "data_source_id", "data_source_id": data_source_id },
        "properties": remote_properties(node),
        "icon": {
            "type": "emoji",
            "emoji": if node.is_folder() { "📁" } else { "📄" }
        }
    });
    let value = parse_json_response(
        http.request(Method::POST, format!("{NOTION_BASE_URL}/pages"))
            .json(&body)
            .send()
            .await
            .context("创建云盘索引页面失败")?,
        "创建云盘索引页面",
    )
    .await?;

    let page_id = value
        .get("id")
        .and_then(Value::as_str)
        .context("Notion 未返回云盘页面 ID")?
        .to_string();
    let page_url = value
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| Some(page_url_from_id(&page_id)));
    Ok((page_id, page_url))
}

pub(super) async fn append_file_block(
    http: &NotionHttp,
    page_id: &str,
    upload_id: &str,
    mime_type: &str,
) -> Result<String> {
    let body = json!({ "children": [file_block(upload_id, mime_type)] });
    let value = parse_json_response(
        http.request(
            Method::PATCH,
            format!("{NOTION_BASE_URL}/blocks/{page_id}/children"),
        )
        .json(&body)
        .send()
        .await
        .context("向云盘页面写入文件块失败")?,
        "向云盘页面写入文件块",
    )
    .await?;

    value
        .get("results")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context("Notion 未返回新文件块 ID")
}

pub(super) async fn patch_remote_block_id(
    http: &NotionHttp,
    page_id: &str,
    block_id: &str,
) -> Result<()> {
    let body = json!({
        "properties": {
            "Block ID": rich_text_property(block_id)
        }
    });
    parse_json_response(
        http.request(
            Method::PATCH,
            format!("{NOTION_BASE_URL}/pages/{page_id}"),
        )
        .json(&body)
        .send()
        .await
        .context("更新远端文件块索引失败")?,
        "更新远端文件块索引",
    )
    .await?;
    Ok(())
}

pub(super) async fn patch_remote_node(http: &NotionHttp, node: &DriveNode) -> Result<()> {
    let body = json!({ "properties": remote_properties(node) });
    parse_json_response(
        http.request(
            Method::PATCH,
            format!("{NOTION_BASE_URL}/pages/{}", node.notion_page_id),
        )
        .json(&body)
        .send()
        .await
        .with_context(|| format!("更新远端节点“{}”失败", node.logical_path))?,
        "更新云盘远端索引",
    )
    .await?;
    Ok(())
}

pub(super) async fn trash_remote_page(http: &NotionHttp, page_id: &str) -> Result<()> {
    parse_json_response(
        http.request(
            Method::PATCH,
            format!("{NOTION_BASE_URL}/pages/{page_id}"),
        )
        .json(&json!({ "in_trash": true }))
        .send()
        .await?,
        "清理未完成云盘页面",
    )
    .await?;
    Ok(())
}

pub(super) async fn find_first_file_block(
    http: &NotionHttp,
    page_id: &str,
) -> Result<String> {
    let mut cursor: Option<String> = None;
    loop {
        let mut url = format!("{NOTION_BASE_URL}/blocks/{page_id}/children?page_size=100");
        if let Some(value) = cursor.as_ref() {
            url.push_str("&start_cursor=");
            url.push_str(value);
        }
        let result = parse_json_response(
            http.request(Method::GET, url).send().await?,
            "查找云盘文件块",
        )
        .await?;

        if let Some(blocks) = result.get("results").and_then(Value::as_array) {
            for block in blocks {
                if is_file_block(block) {
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        return Ok(id.to_string());
                    }
                }
            }
        }

        if !result
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            break;
        }
        cursor = result
            .get("next_cursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    anyhow::bail!("该云盘页面中没有可下载的文件块")
}

pub(super) async fn resolve_file_url(http: &NotionHttp, block_id: &str) -> Result<String> {
    let value = parse_json_response(
        http.request(
            Method::GET,
            format!("{NOTION_BASE_URL}/blocks/{block_id}"),
        )
        .send()
        .await
        .context("读取文件块失败")?,
        "读取文件块",
    )
    .await?;
    extract_file_url(&value).context("Notion 文件块没有返回可下载地址")
}

fn retrieve_data_sources(value: &Value) -> Option<String> {
    value
        .get("data_sources")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

async fn retrieve_first_data_source(http: &NotionHttp, database_id: &str) -> Result<String> {
    let value = parse_json_response(
        http.request(
            Method::GET,
            format!("{NOTION_BASE_URL}/databases/{database_id}"),
        )
        .send()
        .await
        .context("读取新建数据库失败")?,
        "读取新建数据库",
    )
    .await?;
    retrieve_data_sources(&value).context("新建数据库未返回 data_source_id")
}

fn drive_schema() -> Value {
    json!({
        "Name": { "title": {} },
        "Node ID": { "rich_text": {} },
        "Parent ID": { "rich_text": {} },
        "Node Type": {
            "select": { "options": [
                { "name": "file", "color": "blue" },
                { "name": "folder", "color": "yellow" }
            ] }
        },
        "Path": { "rich_text": {} },
        "MIME": { "rich_text": {} },
        "Size": { "number": { "format": "number" } },
        "SHA-256": { "rich_text": {} },
        "Status": {
            "select": { "options": [
                { "name": "active", "color": "green" },
                { "name": "trashed", "color": "gray" }
            ] }
        },
        "Version": { "number": { "format": "number" } },
        "File Upload ID": { "rich_text": {} },
        "Block ID": { "rich_text": {} },
        "Original Path": { "rich_text": {} },
        "Created At": { "date": {} },
        "Modified At": { "date": {} }
    })
}

fn remote_page_to_node(page: &Value) -> Result<Option<DriveNode>> {
    if page.get("in_trash").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(None);
    }
    let properties = page
        .get("properties")
        .and_then(Value::as_object)
        .context("远端索引页面缺少 properties")?;
    let id = property_text(properties, "Node ID");
    if id.trim().is_empty() {
        return Ok(None);
    }

    let page_id = page
        .get("id")
        .and_then(Value::as_str)
        .context("远端索引页面缺少 page_id")?
        .to_string();
    let parent_id = property_text(properties, "Parent ID");
    let created_at = property_date(properties, "Created At")
        .or_else(|| page.get("created_time").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    let modified_at = property_date(properties, "Modified At")
        .or_else(|| page.get("last_edited_time").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| created_at.clone());

    Ok(Some(DriveNode {
        id,
        parent_id: empty_to_none(parent_id),
        node_type: property_select(properties, "Node Type")
            .unwrap_or_else(|| "file".to_string()),
        name: property_title(properties, "Name"),
        logical_path: property_text(properties, "Path"),
        mime_type: empty_to_none(property_text(properties, "MIME")),
        size: property_number(properties, "Size").max(0.0) as u64,
        sha256: empty_to_none(property_text(properties, "SHA-256")),
        notion_page_id: page_id.clone(),
        notion_page_url: page
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| Some(page_url_from_id(&page_id))),
        notion_block_id: empty_to_none(property_text(properties, "Block ID")),
        file_upload_id: empty_to_none(property_text(properties, "File Upload ID")),
        status: property_select(properties, "Status")
            .unwrap_or_else(|| "active".to_string()),
        version: property_number(properties, "Version").max(1.0) as i64,
        original_path: empty_to_none(property_text(properties, "Original Path")),
        created_at,
        modified_at,
    }))
}

fn remote_properties(node: &DriveNode) -> Value {
    json!({
        "Name": {
            "title": [{ "type": "text", "text": { "content": node.name } }]
        },
        "Node ID": rich_text_property(&node.id),
        "Parent ID": rich_text_property(node.parent_id.as_deref().unwrap_or("")),
        "Node Type": { "select": { "name": node.node_type } },
        "Path": rich_text_property(&node.logical_path),
        "MIME": rich_text_property(node.mime_type.as_deref().unwrap_or("")),
        "Size": { "number": node.size },
        "SHA-256": rich_text_property(node.sha256.as_deref().unwrap_or("")),
        "Status": { "select": { "name": node.status } },
        "Version": { "number": node.version },
        "File Upload ID": rich_text_property(node.file_upload_id.as_deref().unwrap_or("")),
        "Block ID": rich_text_property(node.notion_block_id.as_deref().unwrap_or("")),
        "Original Path": rich_text_property(node.original_path.as_deref().unwrap_or("")),
        "Created At": { "date": { "start": node.created_at } },
        "Modified At": { "date": { "start": node.modified_at } }
    })
}

fn rich_text_property(content: &str) -> Value {
    if content.trim().is_empty() {
        json!({ "rich_text": [] })
    } else {
        json!({
            "rich_text": [{
                "type": "text",
                "text": { "content": truncate_chars(content, 1900) }
            }]
        })
    }
}

fn extract_file_url(block: &Value) -> Option<String> {
    let block_type = block.get("type")?.as_str()?;
    if !matches!(block_type, "file" | "image" | "video" | "audio" | "pdf") {
        return None;
    }
    let body = block.get(block_type)?;
    body.pointer("/file/url")
        .or_else(|| body.pointer("/external/url"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn is_file_block(block: &Value) -> bool {
    matches!(
        block.get("type").and_then(Value::as_str),
        Some("file" | "image" | "video" | "audio" | "pdf")
    )
}

fn property_title(properties: &Map<String, Value>, name: &str) -> String {
    properties
        .get(name)
        .and_then(|value| value.get("title"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("plain_text"))
        .and_then(Value::as_str)
        .unwrap_or("未命名")
        .to_string()
}

fn property_text(properties: &Map<String, Value>, name: &str) -> String {
    properties
        .get(name)
        .and_then(|value| value.get("rich_text"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("plain_text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn property_select(properties: &Map<String, Value>, name: &str) -> Option<String> {
    properties
        .get(name)
        .and_then(|value| value.get("select"))
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn property_number(properties: &Map<String, Value>, name: &str) -> f64 {
    properties
        .get(name)
        .and_then(|value| value.get("number"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

fn property_date(properties: &Map<String, Value>, name: &str) -> Option<String> {
    properties
        .get(name)
        .and_then(|value| value.get("date"))
        .and_then(|value| value.get("start"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn empty_to_none(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::{extract_file_url, remote_page_to_node};
    use serde_json::json;

    #[test]
    fn extracts_signed_file_url() {
        let block = json!({
            "type": "file",
            "file": {
                "type": "file",
                "file": { "url": "https://example.com/file", "expiry_time": "x" }
            }
        });
        assert_eq!(
            extract_file_url(&block).as_deref(),
            Some("https://example.com/file")
        );
    }

    #[test]
    fn parses_remote_node_properties() {
        let page = json!({
            "id": "page-1",
            "url": "https://notion.so/page-1",
            "created_time": "2026-08-03T00:00:00Z",
            "last_edited_time": "2026-08-03T00:00:00Z",
            "properties": {
                "Name": { "title": [{ "plain_text": "docs" }] },
                "Node ID": { "rich_text": [{ "plain_text": "node-1" }] },
                "Parent ID": { "rich_text": [] },
                "Node Type": { "select": { "name": "folder" } },
                "Path": { "rich_text": [{ "plain_text": "/docs" }] },
                "MIME": { "rich_text": [] },
                "Size": { "number": 0 },
                "SHA-256": { "rich_text": [] },
                "Status": { "select": { "name": "active" } },
                "Version": { "number": 1 },
                "File Upload ID": { "rich_text": [] },
                "Block ID": { "rich_text": [] },
                "Original Path": { "rich_text": [] },
                "Created At": { "date": { "start": "2026-08-03T00:00:00Z" } },
                "Modified At": { "date": { "start": "2026-08-03T00:00:00Z" } }
            }
        });
        let node = remote_page_to_node(&page).unwrap().unwrap();
        assert_eq!(node.id, "node-1");
        assert!(node.is_folder());
        assert_eq!(node.logical_path, "/docs");
    }
}
