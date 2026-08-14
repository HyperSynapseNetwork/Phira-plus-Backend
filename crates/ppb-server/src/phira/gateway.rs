//! Typed Phira API gateway (charts/records/users proxy) with TTL cache,
//! per-key single-flight dedup, rate limit, and retry/backoff (design §15.8).

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::client::PhiraError;
use crate::middleware::rate_limit::RateLimiter;

const MAX_RETRIES: u32 = 2;

struct CachedValue {
    value: Value,
    expires: Instant,
}

struct CachedFile {
    bytes: Vec<u8>,
    content_type: String,
    expires: Instant,
}

/// Typed gateway over Phira public data endpoints.
#[derive(Clone)]
pub struct PhiraGateway {
    http: reqwest::Client,
    base_url: String,
    cache: Arc<DashMap<String, CachedValue>>,
    file_cache: Arc<DashMap<String, CachedFile>>,
    locks: Arc<DashMap<String, Arc<Mutex<()>>>>,
    rate: Arc<RateLimiter>,
    ttl_secs: i64,
    rate_per_minute: u32,
}

impl PhiraGateway {
    pub fn new(base_url: &str, timeout_ms: u64, ttl_secs: i64, rate_per_minute: u32) -> Result<Self, PhiraError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| PhiraError::Unavailable(e.to_string()))?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            cache: Arc::new(DashMap::new()),
            file_cache: Arc::new(DashMap::new()),
            locks: Arc::new(DashMap::new()),
            rate: Arc::new(RateLimiter::new()),
            ttl_secs,
            rate_per_minute,
        })
    }

    /// GET a JSON resource with cache + dedup + rate-limit + retry.
    pub async fn get_json(&self, path: &str, query: &[(&str, String)]) -> Result<Value, PhiraError> {
        let key = cache_key(path, query);
        if let Some(cv) = self.cache.get(&key) {
            if Instant::now() < cv.expires {
                return Ok(cv.value.clone());
            }
        }
        self.rate
            .check(&format!("phira:{path}"), self.rate_per_minute)
            .map_err(|_| PhiraError::RateLimited)?;

        let lock = self
            .locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Double-check cache after acquiring the single-flight lock.
        if let Some(cv) = self.cache.get(&key) {
            if Instant::now() < cv.expires {
                return Ok(cv.value.clone());
            }
        }

        let value = self.fetch_with_retry(path, query).await?;
        self.cache.insert(
            key,
            CachedValue {
                value: value.clone(),
                expires: Instant::now() + Duration::from_secs(self.ttl_secs.max(1) as u64),
            },
        );
        Ok(value)
    }

    async fn fetch_with_retry(&self, path: &str, query: &[(&str, String)]) -> Result<Value, PhiraError> {
        let url = format!("{}/{path}", self.base_url);
        let mut attempt = 0u32;
        loop {
            let result = self.http.get(&url).query(query).send().await;
            match result {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp
                        .text()
                        .await
                        .map_err(|e| PhiraError::Unavailable(e.to_string()))?;
                    if status.is_success() {
                        return serde_json::from_str::<Value>(&text)
                            .map_err(|_| PhiraError::Api(format!("unexpected payload for {path}")));
                    }
                    if status.as_u16() == 404 {
                        return Err(PhiraError::Api(format!("{path} not found")));
                    }
                    if status.is_server_error() && attempt < MAX_RETRIES {
                        attempt += 1;
                        tokio::time::sleep(Duration::from_millis(200 * attempt as u64)).await;
                        continue;
                    }
                    return Err(PhiraError::Api(format!("{path} failed: status {status}")));
                }
                Err(e) => {
                    if attempt < MAX_RETRIES && is_transient(&e) {
                        attempt += 1;
                        tokio::time::sleep(Duration::from_millis(200 * attempt as u64)).await;
                        continue;
                    }
                    return Err(PhiraError::Unavailable(e.to_string()));
                }
            }
        }
    }

    // ── Typed methods (charts) ──────────────────────────────────

    pub async fn chart_list(
        &self,
        page: i64,
        page_num: i64,
        search: Option<&str>,
        chart_type: Option<i64>,
        rating_min: Option<f64>,
        rating_max: Option<f64>,
        tags: Option<&str>,
        order: Option<&str>,
    ) -> Result<Value, PhiraError> {
        let mut q: Vec<(&str, String)> = vec![
            ("page", page.to_string()),
            ("pageNum", page_num.to_string()),
        ];
        if let Some(s) = search {
            q.push(("search", s.to_string()));
        }
        if let Some(value) = chart_type {
            q.push(("type", value.to_string()));
        }
        if rating_min.is_some() || rating_max.is_some() {
            q.push((
                "rating",
                format!(
                    "{},{}",
                    rating_min.map(|v| v.to_string()).unwrap_or_default(),
                    rating_max.map(|v| v.to_string()).unwrap_or_default()
                ),
            ));
        }
        if let Some(value) = tags.filter(|value| !value.trim().is_empty()) {
            q.push(("tags", value.to_string()));
        }
        if let Some(value) = order.filter(|value| !value.trim().is_empty()) {
            q.push(("order", value.to_string()));
        }
        self.get_json("chart", &q).await
    }

    pub async fn chart(&self, id: i64) -> Result<Value, PhiraError> {
        self.get_json(&format!("chart/{id}"), &[]).await
    }

    pub async fn chart_multi(&self, ids: &[i64]) -> Result<Value, PhiraError> {
        let ids: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
        self.get_json("chart/multi-get", &[("ids", ids.join(","))]).await
    }

    pub async fn chart_popular(&self, page_num: i64) -> Result<Value, PhiraError> {
        self.get_json("chart/popular", &[("pageNum", page_num.to_string())]).await
    }

    // ── Typed methods (records) ─────────────────────────────────

    pub async fn record_query(&self, chart_id: i64, page: i64, page_num: i64) -> Result<Value, PhiraError> {
        self.get_json(
            &format!("record/query/{chart_id}"),
            &[
                ("page", page.to_string()),
                ("pageNum", page_num.to_string()),
            ],
        )
        .await
    }

    /// `GET /record?player=...` — a player's recent records (§15.2).
    pub async fn record_query_player(
        &self,
        player: i64,
        page: i64,
        page_num: i64,
    ) -> Result<Value, PhiraError> {
        self.get_json(
            "record",
            &[
                ("player", player.to_string()),
                ("page", page.to_string()),
                ("pageNum", page_num.to_string()),
            ],
        )
        .await
    }

    pub async fn record_list15(&self, chart_id: i64) -> Result<Value, PhiraError> {
        self.get_json(&format!("record/list15/{chart_id}"), &[]).await
    }

    pub async fn record_multi(&self, ids: &[i64]) -> Result<Value, PhiraError> {
        let ids: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
        self.get_json("record/multi-get", &[("ids", ids.join(","))]).await
    }

    pub async fn record_get_pool(&self, user_id: i64) -> Result<Value, PhiraError> {
        self.get_json(&format!("record/get-pool/{user_id}"), &[]).await
    }

    pub async fn record(&self, id: i64) -> Result<Value, PhiraError> {
        self.get_json(&format!("record/{id}"), &[]).await
    }

    // ── Typed methods (users) ───────────────────────────────────

    pub async fn user(&self, id: i64) -> Result<Value, PhiraError> {
        self.get_json(&format!("user/{id}"), &[]).await
    }

    pub async fn user_stats(&self, id: i64) -> Result<Value, PhiraError> {
        self.get_json(&format!("user/{id}/stats"), &[]).await
    }

    pub async fn users_search(&self, search: &str, page: i64, page_num: i64) -> Result<Value, PhiraError> {
        self.get_json(
            "user",
            &[
                ("search", search.to_string()),
                ("page", page.to_string()),
                ("pageNum", page_num.to_string()),
            ],
        )
        .await
    }

    /// Fetch a chart file (design §12.7 preview fallback) with a TTL byte cache.
    /// Used only when the browser cannot download the Phira CDN file directly.
    pub async fn fetch_chart_file(&self, chart_id: i64) -> Result<(Vec<u8>, String), PhiraError> {
        let key = format!("chart-file:{chart_id}");
        if let Some(cf) = self.file_cache.get(&key) {
            if Instant::now() < cf.expires {
                return Ok((cf.bytes.clone(), cf.content_type.clone()));
            }
        }
        let chart = self.chart(chart_id).await?;
        let file_url = chart
            .get("file")
            .and_then(Value::as_str)
            .ok_or_else(|| PhiraError::Api(format!("chart {chart_id} has no file field")))?;
        let resp = self
            .http
            .get(file_url)
            .send()
            .await
            .map_err(|e| PhiraError::Unavailable(e.to_string()))?;
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| PhiraError::Unavailable(e.to_string()))?
            .to_vec();
        if !status.is_success() {
            return Err(PhiraError::Api(format!("chart file download failed: {status}")));
        }
        self.file_cache.insert(
            key,
            CachedFile {
                bytes: bytes.clone(),
                content_type: content_type.clone(),
                expires: Instant::now() + Duration::from_secs(self.ttl_secs.max(1) as u64),
            },
        );
        Ok((bytes, content_type))
    }

    /// Clear the caches (e.g., after a long outage).
    pub fn clear_cache(&self) {
        self.cache.clear();
        self.file_cache.clear();
    }

    /// Cache stats for /metrics.
    pub fn cache_stats(&self) -> serde_json::Value {
        json!({ "entries": self.cache.len(), "inflight": self.locks.len() })
    }
}

fn cache_key(path: &str, query: &[(&str, String)]) -> String {
    let mut parts: Vec<String> = query.iter().map(|(k, v)| format!("{k}={v}")).collect();
    parts.sort();
    format!("{path}?{}", parts.join("&"))
}

fn is_transient(e: &reqwest::Error) -> bool {
    e.is_timeout() || e.is_connect()
}

/// Merge a Phira error into an ApiError (reuse existing mapping + rate limit).
pub fn phira_gateway_error(e: PhiraError) -> crate::error::ApiError {
    match e {
        PhiraError::RateLimited => crate::error::ApiError::new(
            crate::error::ErrorCode::RateLimited,
            "Phira API rate limited",
        ),
        PhiraError::Unavailable(m) | PhiraError::Api(m) | PhiraError::Other(m) => {
            crate::error::ApiError::new(crate::error::ErrorCode::PhiraApiUnavailable, m)
        }
        PhiraError::InvalidCredentials => crate::error::ApiError::new(
            crate::error::ErrorCode::PhiraAuthFailed,
            "invalid credentials",
        ),
        PhiraError::ReauthRequired(m) => crate::error::ApiError::with_details(
            crate::error::ErrorCode::PhiraReauthRequired,
            "需要重新验证 Phira 身份",
            serde_json::json!({ "reason": m }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc as StdArc;

    /// Spin up a tiny axum server that counts upstream hits and returns a
    /// canned `chart/popular` payload. The listener is bound by tokio before
    /// we return, so the gateway can connect immediately.
    async fn mock_server(counter: StdArc<AtomicUsize>) -> String {
        let app = axum::Router::new().route(
            "/chart/popular",
            axum::routing::get({
                let c = counter.clone();
                move || {
                    let c = c.clone();
                    async move {
                        c.fetch_add(1, Ordering::SeqCst);
                        axum::Json(serde_json::json!({ "results": [ { "id": 1 }, { "id": 2 } ] }))
                    }
                }
            }),
        );
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(tcp, app).await;
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn gateway_ttl_cache_dedups_upstream() {
        let counter = StdArc::new(AtomicUsize::new(0));
        let base = mock_server(counter.clone()).await;
        let gw = PhiraGateway::new(&base, 1000, 60, 100).unwrap();

        let first = gw.chart_popular(10).await.unwrap();
        let second = gw.chart_popular(10).await.unwrap();
        let third = gw.chart_popular(10).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(second, third);
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "TTL cache must dedup upstream requests"
        );
    }

    #[test]
    fn rate_limited_error_maps_to_ratelimit() {
        let api = phira_gateway_error(PhiraError::RateLimited);
        assert_eq!(api.code, crate::error::ErrorCode::RateLimited);
    }

    #[test]
    fn cache_key_is_sorted() {
        let q = [
            ("page", "2".to_string()),
            ("pageNum", "20".to_string()),
        ];
        assert_eq!(cache_key("chart", &q), "chart?page=2&pageNum=20");
    }

    #[test]
    fn chart_ids_extracted_from_popular_payload() {
        let v = serde_json::json!({ "results": [ { "id": 5 }, { "id": 9 }, {} ] });
        let ids = crate::phira::aggregator::extract_chart_ids(&v);
        assert_eq!(ids, vec![5, 9]);
    }
}
