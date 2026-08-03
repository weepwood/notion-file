use anyhow::{Context, Result};
use reqwest::{multipart, Client, Method, RequestBuilder, Response, StatusCode};
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::time::sleep;

pub const NOTION_VERSION: &str = "2026-03-11";
const REQUEST_INTERVAL: Duration = Duration::from_millis(350);
const MAX_ATTEMPTS: usize = 5;
const MAX_BACKOFF: Duration = Duration::from_secs(16);

#[derive(Clone)]
pub struct NotionHttp {
    client: Client,
    token: Arc<str>,
}

pub struct NotionRequest {
    http: NotionHttp,
    method: Method,
    url: String,
    json_body: Option<Value>,
}

struct RateLimiter {
    next_allowed: Mutex<Instant>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetryPolicy {
    /// GET、HEAD、OPTIONS、DELETE 等可安全重复执行的请求。
    Idempotent,
    /// POST、PATCH、文件上传等写请求。只在明确未执行的情况下重试，
    /// 避免响应丢失后重复创建页面或重复追加块。
    RateLimitOnly,
}

static GLOBAL_LIMITER: OnceLock<Arc<RateLimiter>> = OnceLock::new();

impl NotionHttp {
    pub fn new(token: String) -> Result<Self> {
        let client = Client::builder()
            .user_agent("notion-file/0.3.0")
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(600))
            .build()
            .context("无法初始化 Notion HTTP 客户端")?;
        Ok(Self {
            client,
            token: Arc::<str>::from(token),
        })
    }

    pub fn request(&self, method: Method, url: impl Into<String>) -> NotionRequest {
        NotionRequest {
            http: self.clone(),
            method,
            url: url.into(),
            json_body: None,
        }
    }

    pub async fn send_multipart(
        &self,
        url: impl Into<String>,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
        part_number: Option<u64>,
    ) -> Result<Response> {
        let url = url.into();
        let file_name = file_name.to_string();
        let mime_type = mime_type.to_string();
        let client = self.client.clone();
        let token = self.token.clone();

        execute_with_retry(
            move || {
                let file = multipart::Part::bytes(bytes.clone())
                    .file_name(file_name.clone())
                    .mime_str(&mime_type)?;
                let mut form = multipart::Form::new().part("file", file);
                if let Some(part_number) = part_number {
                    form = form.text("part_number", part_number.to_string());
                }
                Ok(client
                    .post(&url)
                    .bearer_auth(token.as_ref())
                    .header("Notion-Version", NOTION_VERSION)
                    .header("Accept", "application/json")
                    .multipart(form))
            },
            RetryPolicy::RateLimitOnly,
        )
        .await
    }
}

impl NotionRequest {
    pub fn json(mut self, body: &Value) -> Self {
        self.json_body = Some(body.clone());
        self
    }

    pub async fn send(self) -> Result<Response> {
        let client = self.http.client.clone();
        let token = self.http.token.clone();
        let method = self.method;
        let retry_policy = retry_policy_for_method(&method);
        let url = self.url;
        let json_body = self.json_body;

        execute_with_retry(
            move || {
                let mut builder = client
                    .request(method.clone(), &url)
                    .bearer_auth(token.as_ref())
                    .header("Notion-Version", NOTION_VERSION)
                    .header("Accept", "application/json");
                if let Some(body) = json_body.as_ref() {
                    builder = builder.json(body);
                }
                Ok(builder)
            },
            retry_policy,
        )
        .await
    }
}

pub async fn parse_json_response(response: Response, context: &str) -> Result<Value> {
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let text = response.text().await.unwrap_or_default();

    if !status.is_success() {
        let parsed = serde_json::from_str::<Value>(&text).ok();
        let code = parsed
            .as_ref()
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str);
        let message = parsed
            .as_ref()
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str)
            .unwrap_or(text.trim());
        let request_suffix = request_id
            .as_deref()
            .map(|value| format!("，request_id={value}"))
            .unwrap_or_default();
        let code_prefix = code.map(|value| format!("{value}: ")).unwrap_or_default();
        anyhow::bail!(
            "{context}失败（HTTP {}）：{code_prefix}{message}{request_suffix}",
            status.as_u16()
        );
    }

    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).with_context(|| format!("{context}返回了无效 JSON"))
}

async fn execute_with_retry<F>(factory: F, retry_policy: RetryPolicy) -> Result<Response>
where
    F: Fn() -> Result<RequestBuilder>,
{
    let mut last_network_error = None;

    for attempt in 0..MAX_ATTEMPTS {
        limiter().acquire().await;
        let builder = factory()?;

        match builder.send().await {
            Ok(response) => {
                if !should_retry_status(response.status(), retry_policy)
                    || attempt + 1 == MAX_ATTEMPTS
                {
                    return Ok(response);
                }

                let delay = retry_delay(&response, attempt);
                sleep(delay).await;
            }
            Err(error) => {
                let can_retry = retry_policy == RetryPolicy::Idempotent || error.is_connect();
                if !can_retry {
                    return Err(error).context(
                        "Notion 写请求发生网络错误；为避免重复创建页面或重复写入，未自动重试",
                    );
                }
                if attempt + 1 == MAX_ATTEMPTS {
                    return Err(error).context("Notion API 网络请求失败，已达到最大重试次数");
                }
                last_network_error = Some(error);
                sleep(exponential_backoff(attempt)).await;
            }
        }
    }

    Err(last_network_error
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow::anyhow!("Notion API 请求重试失败")))
}

impl RateLimiter {
    async fn acquire(&self) {
        let delay = {
            let mut next_allowed = self.next_allowed.lock().await;
            let now = Instant::now();
            let scheduled = (*next_allowed).max(now);
            *next_allowed = scheduled + REQUEST_INTERVAL;
            scheduled.saturating_duration_since(now)
        };

        if !delay.is_zero() {
            sleep(delay).await;
        }
    }
}

fn limiter() -> &'static Arc<RateLimiter> {
    GLOBAL_LIMITER.get_or_init(|| {
        Arc::new(RateLimiter {
            next_allowed: Mutex::new(Instant::now()),
        })
    })
}

fn retry_policy_for_method(method: &Method) -> RetryPolicy {
    if matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::DELETE
    ) {
        RetryPolicy::Idempotent
    } else {
        RetryPolicy::RateLimitOnly
    }
}

fn should_retry_status(status: StatusCode, retry_policy: RetryPolicy) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || (retry_policy == RetryPolicy::Idempotent && is_transient_server_status(status))
}

fn is_transient_server_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}

fn retry_delay(response: &Response, attempt: usize) -> Duration {
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        if let Some(seconds) = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
        {
            return Duration::from_secs(seconds);
        }
    }
    exponential_backoff(attempt)
}

fn exponential_backoff(attempt: usize) -> Duration {
    let seconds = 1_u64 << attempt.min(4);
    Duration::from_secs(seconds).min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::{
        exponential_backoff, retry_policy_for_method, should_retry_status, RetryPolicy,
    };
    use reqwest::{Method, StatusCode};
    use std::time::Duration;

    #[test]
    fn classifies_request_idempotency() {
        assert_eq!(retry_policy_for_method(&Method::GET), RetryPolicy::Idempotent);
        assert_eq!(retry_policy_for_method(&Method::DELETE), RetryPolicy::Idempotent);
        assert_eq!(
            retry_policy_for_method(&Method::POST),
            RetryPolicy::RateLimitOnly
        );
        assert_eq!(
            retry_policy_for_method(&Method::PATCH),
            RetryPolicy::RateLimitOnly
        );
    }

    #[test]
    fn retries_rate_limits_for_all_requests() {
        assert!(should_retry_status(
            StatusCode::TOO_MANY_REQUESTS,
            RetryPolicy::Idempotent
        ));
        assert!(should_retry_status(
            StatusCode::TOO_MANY_REQUESTS,
            RetryPolicy::RateLimitOnly
        ));
    }

    #[test]
    fn retries_server_errors_only_for_idempotent_requests() {
        assert!(should_retry_status(
            StatusCode::SERVICE_UNAVAILABLE,
            RetryPolicy::Idempotent
        ));
        assert!(!should_retry_status(
            StatusCode::SERVICE_UNAVAILABLE,
            RetryPolicy::RateLimitOnly
        ));
        assert!(!should_retry_status(
            StatusCode::BAD_REQUEST,
            RetryPolicy::Idempotent
        ));
    }

    #[test]
    fn uses_capped_exponential_backoff() {
        assert_eq!(exponential_backoff(0), Duration::from_secs(1));
        assert_eq!(exponential_backoff(3), Duration::from_secs(8));
        assert_eq!(exponential_backoff(10), Duration::from_secs(16));
    }
}
