//! openfda-cli-rs: Rust port of lesterppo/openfda-cli (Python, v0.1.0).
//!
//! Pure logic (query sanitising, dotted-path traversal, field projection,
//! aggregation, `--output` filtering, error classification, cache keying) lives
//! here so it is unit-testable without the network. The HTTP client and the
//! SQLite cache live here too; the CLI argument handling lives in `main.rs`.

use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const VERSION: &str = "0.1.0";
pub const DEFAULT_BASE_URL: &str = "https://api.fda.gov";
pub const CACHE_TTL_HOURS: f64 = 24.0;

/// Characters the Python original strips from queries:
/// control characters \x00-\x08, \x0b, \x0c, \x0e-\x1f (note: \x09 tab,
/// \x0a newline and \x0d carriage return are NOT stripped by the regex,
/// only by the surrounding `.strip()`).
pub fn sanitize_query(query: &str) -> String {
    let stripped: String = query
        .chars()
        .filter(|c| {
            let u = *c as u32;
            !(u <= 0x08 || u == 0x0b || u == 0x0c || (0x0e..=0x1f).contains(&u))
        })
        .collect();
    stripped.trim().to_string()
}

/// Split a `--fields` value the way the original does: comma-separated,
/// whitespace-trimmed, empty items dropped. `None` stays `None`.
pub fn parse_fields(fields: Option<&str>) -> Option<Vec<String>> {
    fields.map(|s| {
        s.split(',')
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty())
            .collect()
    })
}

/// Port of `models._traverse`: walk a dotted path into nested dicts/lists
/// and return every value found.
///
/// * dict: look the part up; missing key -> no values.
/// * list: a numeric part indexes one element (out of range -> no values);
///   a non-numeric part walks *every* element with the same (unconsumed) path.
/// * a `null` value is never returned; an empty path returns the object
///   itself unless it is null.
pub fn traverse(obj: &Value, parts: &[&str]) -> Vec<Value> {
    if parts.is_empty() {
        return if obj.is_null() { vec![] } else { vec![obj.clone()] };
    }
    let part = parts[0];
    let rest = &parts[1..];
    match obj {
        Value::Object(map) => match map.get(part) {
            Some(v) => {
                if rest.is_empty() {
                    if v.is_null() { vec![] } else { vec![v.clone()] }
                } else {
                    traverse(v, rest)
                }
            }
            None => vec![],
        },
        Value::Array(items) => {
            if let Ok(idx) = part.parse::<usize>() {
                match items.get(idx) {
                    Some(v) => traverse(v, rest),
                    None => vec![],
                }
            } else {
                let mut out = Vec::new();
                for item in items {
                    out.extend(traverse(item, parts));
                }
                out
            }
        }
        _ => vec![],
    }
}

fn traverse_path(obj: &Value, path: &str) -> Vec<Value> {
    let parts: Vec<&str> = path.split('.').collect();
    traverse(obj, &parts)
}

/// Port of `models.project_dict`: for each field path, keep the found value
/// under the full dotted key. No values -> field omitted. Several values ->
/// deduplicated (by JSON text, order preserved) into a list.
pub fn project_dict(record: &Value, fields: &[String]) -> Value {
    let mut out = Map::new();
    for field in fields {
        let vals = traverse_path(record, field);
        if vals.is_empty() {
            continue;
        }
        if vals.len() == 1 {
            out.insert(field.clone(), vals.into_iter().next().unwrap());
        } else {
            let mut seen: Vec<String> = Vec::new();
            let mut deduped: Vec<Value> = Vec::new();
            for v in vals {
                let key = serde_json::to_string(&v).unwrap_or_default();
                if !seen.contains(&key) {
                    seen.push(key);
                    deduped.push(v);
                }
            }
            out.insert(field.clone(), Value::Array(deduped));
        }
    }
    Value::Object(out)
}

/// Port of `models.project_results`.
pub fn project_results(results: &[Value], fields: Option<&[String]>) -> Vec<Value> {
    match fields {
        None => results.to_vec(),
        Some(f) => results.iter().map(|r| project_dict(r, f)).collect(),
    }
}

/// Port of `models.aggregate_field`: count string values at `field_path`,
/// return `[{"value", "count"}]` sorted by count descending. Ties keep
/// first-encounter order (stable sort, matching `Counter.most_common`).
/// `top_n`: `None` -> all; `Some(n <= 0)` -> empty (as in the original).
pub fn aggregate_field(results: &[Value], field_path: &str, top_n: Option<i64>) -> Vec<Value> {
    let mut counts: HashMap<String, i64> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for row in results {
        for v in traverse_path(row, field_path) {
            if let Value::String(s) = v {
                if !counts.contains_key(&s) {
                    order.push(s.clone());
                }
                *counts.entry(s).or_insert(0) += 1;
            }
        }
    }
    let mut ranked: Vec<(String, i64)> = order
        .into_iter()
        .map(|s| {
            let c = counts[&s];
            (s, c)
        })
        .collect();
    // Stable sort: equal counts keep first-encounter order.
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    let ranked = match top_n {
        None => ranked,
        Some(n) if n <= 0 => Vec::new(),
        Some(n) => ranked.into_iter().take(n as usize).collect(),
    };
    ranked
        .into_iter()
        .map(|(value, count)| json!({"value": value, "count": count}))
        .collect()
}

/// Port of `cli._filter_output`: build `{"success": ...}` plus only the
/// dot-paths named in `spec` (comma-separated). A path that cannot be
/// walked (missing key, or a step through a non-object such as a list)
/// is silently skipped, exactly as in the original. Nested paths recreate
/// their intermediate objects.
pub fn filter_output(data: &Value, spec: &str) -> Value {
    let success = data
        .get("success")
        .cloned()
        .unwrap_or(Value::Bool(true));
    if spec.is_empty() {
        return data.clone();
    }
    let mut result = Map::new();
    result.insert("success".to_string(), success);
    for item in spec.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let parts: Vec<&str> = item.split('.').collect();
        // Walk dicts only (Python: val[p] on a list raises TypeError -> skip).
        let mut cur = data;
        let mut found = true;
        for p in &parts {
            match cur {
                Value::Object(map) => match map.get(*p) {
                    Some(v) => cur = v,
                    None => {
                        found = false;
                        break;
                    }
                },
                _ => {
                    found = false;
                    break;
                }
            }
        }
        if !found {
            continue;
        }
        let mut target = &mut result;
        for p in &parts[..parts.len() - 1] {
            let entry = target
                .entry(p.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if !entry.is_object() {
                *entry = Value::Object(Map::new());
            }
            target = entry.as_object_mut().unwrap();
        }
        target.insert(parts[parts.len() - 1].to_string(), cur.clone());
    }
    Value::Object(result)
}

/// Cache key, identical to the original: `{endpoint}|{query}|{limit}`.
/// Note the key uses the *raw* query (pre-sanitising) and ignores
/// `page` and `fields`, exactly as in the Python original.
pub fn cache_key(endpoint: &str, query: &str, limit: i64) -> String {
    format!("{}|{}|{}", endpoint, query, limit)
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Error type mirroring the original's `FDAClientError` subclasses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdaError {
    pub message: String,
    pub error_type: String,
}

impl FdaError {
    pub fn new(message: impl Into<String>, error_type: impl Into<String>) -> Self {
        FdaError {
            message: message.into(),
            error_type: error_type.into(),
        }
    }
    pub fn rate_limit() -> Self {
        FdaError::new("Rate limited by FDA API", "rate_limit")
    }
    pub fn not_found() -> Self {
        FdaError::new("FDA endpoint returned 404", "not_found")
    }
    pub fn network(message: impl Into<String>) -> Self {
        FdaError::new(message, "network")
    }
    pub fn validation(message: impl Into<String>) -> Self {
        FdaError::new(message, "validation")
    }
    pub fn api_error(status: u16) -> Self {
        FdaError::new(format!("HTTP {}", status), "api_error")
    }
    pub fn internal(message: impl Into<String>) -> Self {
        FdaError::new(message, "internal")
    }
}

impl fmt::Display for FdaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for FdaError {}

/// Classify an HTTP status the way `client._classify_error` does.
/// Returns `None` for success; a retryable flag for 429 / 5xx.
pub fn classify_status(status: u16) -> Option<(FdaError, bool)> {
    match status {
        429 => Some((FdaError::rate_limit(), true)),
        404 => Some((FdaError::not_found(), false)),
        500..=599 => Some((FdaError::network(format!("FDA server error ({})", status)), true)),
        s => Some((FdaError::api_error(s), false)),
    }
}

/// SQLite cache: same table shape (`cache(key, data, timestamp)`) and
/// 24-hour TTL as the original. The default path differs deliberately:
/// the original resolves `fda_cache.db` relative to its own source tree,
/// which has no meaning for an installed binary, so this port uses
/// `./fda_cache.db` in the working directory, overridable with
/// `OPENFDA_CACHE_PATH`.
pub struct FdaCache {
    conn: rusqlite::Connection,
    pub ttl_hours: f64,
}

impl FdaCache {
    pub fn open(path: &str, ttl_hours: f64) -> Result<Self, rusqlite::Error> {
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; \
             CREATE TABLE IF NOT EXISTS cache \
             (key TEXT PRIMARY KEY, data TEXT, timestamp REAL)",
        )?;
        Ok(FdaCache { conn, ttl_hours })
    }

    pub fn get(&self, endpoint: &str, query: &str, limit: i64) -> Option<Value> {
        let key = cache_key(endpoint, query, limit);
        let cutoff = now_secs() - self.ttl_hours * 3600.0;
        let row: Result<String, _> = self.conn.query_row(
            "SELECT data FROM cache WHERE key = ?1 AND timestamp >= ?2",
            rusqlite::params![key, cutoff],
            |r| r.get(0),
        );
        row.ok().and_then(|s| serde_json::from_str(&s).ok())
    }

    pub fn set(&self, endpoint: &str, query: &str, limit: i64, data: &Value) {
        let key = cache_key(endpoint, query, limit);
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO cache (key, data, timestamp) VALUES (?1, ?2, ?3)",
            rusqlite::params![key, serde_json::to_string(data).unwrap_or_default(), now_secs()],
        );
    }

    pub fn clear_expired(&self) -> usize {
        let cutoff = now_secs() - self.ttl_hours * 3600.0;
        self.conn
            .execute("DELETE FROM cache WHERE timestamp < ?1", rusqlite::params![cutoff])
            .unwrap_or(0)
    }
}

/// HTTP client. Mirrors `client.OpenFDAClient` except for one deliberate
/// improvement, documented in the README: the original decorates `_fetch`
/// with a tenacity retry for `NetworkError`/`RateLimitError`, but `_fetch`
/// only ever raises raw httpx errors, so the retry never fires. This port
/// actually retries network errors, 429s and 5xx up to 3 attempts with
/// 2s/4s backoff, matching the configured (but dead) policy.
pub struct FdaClient {
    pub base_url: String,
    http: reqwest::blocking::Client,
    pub cache: Option<FdaCache>,
}

impl FdaClient {
    pub fn new(cache: Option<FdaCache>) -> Result<Self, FdaError> {
        let base_url = std::env::var("OPENFDA_API_BASE")
            .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| FdaError::internal(e.to_string()))?;
        Ok(FdaClient {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
            cache,
        })
    }

    pub fn endpoint_url(&self, endpoint: &str) -> String {
        let ep = if endpoint.ends_with(".json") {
            endpoint.to_string()
        } else {
            format!("{}.json", endpoint)
        };
        format!("{}/{}", self.base_url, ep)
    }

    fn fetch_once(
        &self,
        endpoint: &str,
        raw_query: &str,
        limit: i64,
        page: i64,
    ) -> Result<Value, FdaError> {
        let sanitized = sanitize_query(raw_query);
        let skip = (page - 1) * limit;
        let mut req = self
            .http
            .get(self.endpoint_url(endpoint))
            .query(&[("limit", limit.to_string()), ("skip", skip.to_string())]);
        if !sanitized.is_empty() {
            req = req.query(&[("search", sanitized)]);
        }
        let resp = req.send().map_err(|e| {
            if e.is_timeout() {
                FdaError::network(format!("Request timed out: {}", e))
            } else if e.is_connect() {
                FdaError::network(format!("Connection failed: {}", e))
            } else {
                FdaError::internal(e.to_string())
            }
        })?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let (err, _retryable) = classify_status(status).unwrap();
            return Err(err);
        }
        resp.json::<Value>()
            .map_err(|e| FdaError::internal(e.to_string()))
    }

    /// Returns the full API response with `results` projected by `fields`.
    /// Cache rules mirror the original: only page 1 is read from / written
    /// to the cache, and a fresh fetch on page 1 is *stored* even when
    /// `use_cache` is false (`--no-cache` bypasses reads, not writes).
    pub fn get_fda_data(
        &self,
        endpoint: &str,
        query: &str,
        limit: i64,
        fields: Option<&[String]>,
        page: i64,
        use_cache: bool,
    ) -> Result<Value, FdaError> {
        if endpoint.is_empty() || !endpoint.contains('/') {
            return Err(FdaError::validation(
                "Endpoint must be in format 'category/subcategory' (e.g. 'drug/event')",
            ));
        }
        if !(1..=100).contains(&limit) {
            return Err(FdaError::validation("Limit must be between 1 and 100"));
        }
        if use_cache && page == 1 {
            if let Some(cache) = &self.cache {
                if let Some(cached) = cache.get(endpoint, query, limit) {
                    return Ok(project_cached(cached, fields));
                }
            }
        }
        // Retry network / 429 / 5xx, 3 attempts, 2s then 4s backoff.
        let mut attempt = 0;
        let data = loop {
            match self.fetch_once(endpoint, query, limit, page) {
                Ok(d) => break d,
                Err(e) => {
                    let retryable = e.error_type == "rate_limit"
                        || (e.error_type == "network"
                            && (e.message.starts_with("Request timed out")
                                || e.message.starts_with("Connection failed")
                                || e.message.starts_with("FDA server error")));
                    attempt += 1;
                    if retryable && attempt < 3 {
                        std::thread::sleep(Duration::from_secs(if attempt == 1 { 2 } else { 4 }));
                        continue;
                    }
                    return Err(e);
                }
            }
        };
        if page == 1 {
            if let Some(cache) = &self.cache {
                cache.set(endpoint, query, limit, &data);
            }
        }
        Ok(project_cached(data, fields))
    }
}

fn project_cached(mut data: Value, fields: Option<&[String]>) -> Value {
    if let Some(f) = fields {
        let results: Vec<Value> = data
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        let projected = project_results(&results, Some(f));
        if let Value::Object(map) = &mut data {
            map.insert("results".to_string(), Value::Array(projected));
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_control_chars_and_trims() {
        assert_eq!(sanitize_query("  aspirin\x00\x07\x0b  "), "aspirin");
        // Tab/newline are not in the stripped set; trim only removes edges.
        assert_eq!(sanitize_query("a\tb"), "a\tb");
        assert_eq!(sanitize_query(""), "");
    }

    #[test]
    fn traverse_dict_list_and_index() {
        let v: Value = serde_json::from_str(
            r#"{"patient":{"reaction":[{"reactionmeddrapt":"NAUSEA"},{"reactionmeddrapt":"HEADACHE"}]},"safetyreportid":"1"}"#,
        )
        .unwrap();
        let vals = traverse_path(&v, "patient.reaction.reactionmeddrapt");
        assert_eq!(vals, vec![json!("NAUSEA"), json!("HEADACHE")]);
        assert_eq!(traverse_path(&v, "patient.reaction.1.reactionmeddrapt"), vec![json!("HEADACHE")]);
        assert_eq!(traverse_path(&v, "patient.reaction.9.reactionmeddrapt"), Vec::<Value>::new());
        assert_eq!(traverse_path(&v, "missing.deep"), Vec::<Value>::new());
        assert_eq!(traverse_path(&v, "safetyreportid"), vec![json!("1")]);
        // Null is never returned.
        let n: Value = serde_json::from_str(r#"{"a":null}"#).unwrap();
        assert_eq!(traverse_path(&n, "a"), Vec::<Value>::new());
    }

    #[test]
    fn project_dict_dedupes_and_omits_missing() {
        // Fan-out across list items dedupes; a list-valued leaf field is
        // returned as one value (the whole list), exactly as in the original.
        let v: Value = serde_json::from_str(
            r#"{"products":[{"brand_name":"A"},{"brand_name":"A"},{"brand_name":"B"}],"openfda":{"brand_name":["A","A"]}}"#,
        )
        .unwrap();
        let out = project_dict(
            &v,
            &[
                "products.brand_name".to_string(),
                "openfda.brand_name".to_string(),
                "nope".to_string(),
            ],
        );
        assert_eq!(out["products.brand_name"], json!(["A", "B"]));
        assert_eq!(out["openfda.brand_name"], json!(["A", "A"]));
        assert!(out.get("nope").is_none());
    }

    #[test]
    fn aggregate_counts_strings_only_and_top() {
        let rows: Vec<Value> = serde_json::from_str(
            r#"[{"r":{"x":"A"}},{"r":{"x":"B"}},{"r":{"x":"A"}},{"r":{"x":3}},{"r":{}}]"#,
        )
        .unwrap();
        let rows: Vec<Value> = rows.iter().map(|r| r["r"].clone()).collect();
        // Re-shape: aggregate over path "x" on each row object.
        let agg = aggregate_field(&rows, "x", None);
        assert_eq!(agg, vec![json!({"value":"A","count":2}), json!({"value":"B","count":1})]);
        assert_eq!(aggregate_field(&rows, "x", Some(1)).len(), 1);
        assert_eq!(aggregate_field(&rows, "x", Some(0)), Vec::<Value>::new());
    }

    #[test]
    fn filter_output_nested_and_missing() {
        let data: Value = serde_json::from_str(
            r#"{"success":true,"data":[1],"meta":{"results":{"total":42},"license":"x"}}"#,
        )
        .unwrap();
        let out = filter_output(&data, "data,meta.results.total");
        assert_eq!(out, json!({"success":true,"data":[1],"meta":{"results":{"total":42}}}));
        // Missing path skipped; list-index path skipped (dict-only walk).
        let out2 = filter_output(&data, "meta.nope,data.0");
        assert_eq!(out2, json!({"success":true}));
        assert_eq!(filter_output(&data, ""), data);
    }

    #[test]
    fn error_classification() {
        assert_eq!(classify_status(429).unwrap().0.error_type, "rate_limit");
        assert_eq!(classify_status(404).unwrap().0.error_type, "not_found");
        assert_eq!(classify_status(503).unwrap().0.error_type, "network");
        assert_eq!(classify_status(400).unwrap().0.error_type, "api_error");
        assert!(classify_status(429).unwrap().1);
        assert!(!classify_status(404).unwrap().1);
    }

    #[test]
    fn cache_key_matches_original_format() {
        assert_eq!(cache_key("drug/label", "openfda.generic_name:aspirin", 5), "drug/label|openfda.generic_name:aspirin|5");
    }

    #[test]
    fn cache_round_trip_and_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.db");
        let cache = FdaCache::open(path.to_str().unwrap(), 24.0).unwrap();
        let data = json!({"results": [{"a": 1}], "meta": {}});
        assert!(cache.get("drug/label", "aspirin", 5).is_none());
        cache.set("drug/label", "aspirin", 5, &data);
        assert_eq!(cache.get("drug/label", "aspirin", 5).unwrap(), data);
        // Different limit -> different key.
        assert!(cache.get("drug/label", "aspirin", 6).is_none());
        // Zero TTL -> immediately expired.
        let cache0 = FdaCache::open(dir.path().join("c0.db").to_str().unwrap(), 0.0).unwrap();
        cache0.set("drug/label", "aspirin", 5, &data);
        assert!(cache0.get("drug/label", "aspirin", 5).is_none());
    }

    #[test]
    fn parse_fields_trims_and_drops_empty() {
        assert_eq!(
            parse_fields(Some(" a , ,b ")).unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(parse_fields(None).is_none());
    }
}
