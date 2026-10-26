//! CLI front end: mirrors `openfda_cli/cli.py` argument-for-argument.
//! Output contract: stdout is JSON only, one object per line.
//! Errors in single-query mode print `{"success":false,"error":...,
//! "error_type":...}` and exit 1. Batch mode exits 0 even when individual
//! entries (or the batch JSON itself) fail — a quirk of the original,
//! replicated deliberately (see README).

use clap::{Arg, ArgAction, Command};
use openfda_cli_rs::{
    aggregate_field, filter_output, parse_fields, FdaCache, FdaClient, FdaError, VERSION,
};
use serde_json::{json, Map, Value};
use std::io::{IsTerminal, Read, Write};

fn output(data: &Value) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{}", serde_json::to_string(data).unwrap_or_default());
    let _ = out.flush();
}

fn error_output(message: &str, error_type: &str) -> Value {
    json!({"success": false, "error": message, "error_type": error_type})
}

/// Read stdin only when it is not a terminal, stripped — like the original.
fn read_stdin_content() -> String {
    if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        if std::io::stdin().read_to_string(&mut buf).is_ok() {
            return buf.trim().to_string();
        }
    }
    String::new()
}

struct Logger {
    session_id: String,
}

impl Logger {
    fn new() -> Self {
        // 12 hex chars, like the original's uuid hex prefix.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let session_id = format!("{:012x}", (nanos ^ (std::process::id() as u128)) & 0xffffffffffff);
        Logger { session_id }
    }
    fn log(&self, level: &str, msg: &str) {
        // Best-effort file log mirroring logs/fda_agent.log; failures ignored
        // so logging can never break the JSON stdout contract.
        if std::fs::create_dir_all("logs").is_ok() {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("logs/fda_agent.log")
            {
                let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                let _ = writeln!(f, "[{}] [{}] [{}] {}", ts, level, self.session_id, msg);
            }
        }
    }
}

fn make_client(logger: &Logger) -> Result<FdaClient, FdaError> {
    let path = std::env::var("OPENFDA_CACHE_PATH").unwrap_or_else(|_| "fda_cache.db".to_string());
    match FdaCache::open(&path, 24.0) {
        Ok(cache) => {
            logger.log("INFO", &format!("Cache at {}", path));
            FdaClient::new(Some(cache))
        }
        Err(e) => {
            // A cache that cannot open must not kill the query: the
            // original would fail here, but a cache failure is not a
            // data failure. Log and continue cacheless.
            logger.log("ERROR", &format!("Cache open failed ({}), continuing without cache", e));
            FdaClient::new(None)
        }
    }
}

/// Single-query execution shared by single and batch modes.
fn execute_query(
    client: &FdaClient,
    endpoint: &str,
    query: &str,
    limit: i64,
    fields: Option<&[String]>,
    page: i64,
    no_cache: bool,
    aggregate: Option<&str>,
    top_n: Option<i64>,
) -> Result<Value, FdaError> {
    let data = client.get_fda_data(endpoint, query, limit, fields, page, !no_cache)?;
    let results: Vec<Value> = data
        .get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let meta = data.get("meta").cloned().unwrap_or(Value::Null);
    let mut result = Map::new();
    result.insert("success".to_string(), Value::Bool(true));
    result.insert("data".to_string(), Value::Array(results.clone()));
    result.insert("meta".to_string(), meta);
    if let Some(agg) = aggregate {
        if !agg.is_empty() {
            let agg_vals = aggregate_field(&results, agg, top_n);
            result.insert(
                "aggregation".to_string(),
                json!({"field": agg, "top": agg_vals}),
            );
        }
    }
    Ok(Value::Object(result))
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(false) => false,
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Bool(true) => true,
    }
}

/// Returns (output value, exit code). Batch JSON errors return exit 0
/// with `success:false` (original quirk); a non-object batch element
/// aborts with an internal error and exit 1 (the original crashes out
/// of `_run_batch` into the same shape via its outer handler).
fn run_batch(
    client: &FdaClient,
    stdin_content: &str,
    global_aggregate: Option<&str>,
    global_top: Option<i64>,
    global_no_cache: bool,
) -> (Value, i32) {
    let parsed: Result<Value, _> = serde_json::from_str(stdin_content);
    let queries = match parsed {
        Err(e) => {
            return (
                json!({"success": false, "error": format!("Invalid batch JSON: {}", e), "error_type": "validation"}),
                0,
            );
        }
        Ok(v) => v,
    };
    let queries = match queries.as_array() {
        Some(a) => a.clone(),
        None => {
            return (
                json!({"success": false, "error": "Batch input must be a JSON array", "error_type": "validation"}),
                0,
            );
        }
    };
    let mut batch_results: Vec<Value> = Vec::new();
    for (idx, q) in queries.iter().enumerate() {
        let obj = match q.as_object() {
            Some(o) => o,
            None => {
                return (
                    error_output("Batch query must be an object", "internal"),
                    1,
                );
            }
        };
        let fields = parse_fields(obj.get("fields").and_then(|f| f.as_str()));
        let endpoint = obj.get("endpoint").and_then(|e| e.as_str()).unwrap_or("");
        let query = obj.get("query").and_then(|e| e.as_str()).unwrap_or("");
        // Non-integer limit/page/top or a non-string aggregate are
        // internal errors per entry, as in the original (TypeError path).
        let limit = match obj.get("limit") {
            None => 10,
            Some(v) => match v.as_i64() {
                Some(n) => n,
                None => {
                    let mut e = error_output("limit must be an integer", "internal");
                    e["id"] = json!(obj.get("id").cloned().unwrap_or(json!(idx)));
                    batch_results.push(e);
                    continue;
                }
            },
        };
        let page = match obj.get("page") {
            None => 1,
            Some(v) => match v.as_i64() {
                Some(n) => n,
                None => {
                    let mut e = error_output("page must be an integer", "internal");
                    e["id"] = json!(obj.get("id").cloned().unwrap_or(json!(idx)));
                    batch_results.push(e);
                    continue;
                }
            },
        };
        let no_cache = match obj.get("no-cache") {
            Some(v) => truthy(v),
            None => global_no_cache,
        };
        let aggregate: Option<String> = match obj.get("aggregate") {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            Some(Value::String(_)) => global_aggregate.map(|s| s.to_string()),
            Some(_) => {
                let mut e = error_output("aggregate must be a string", "internal");
                e["id"] = json!(obj.get("id").cloned().unwrap_or(json!(idx)));
                batch_results.push(e);
                continue;
            }
            None => global_aggregate.map(|s| s.to_string()),
        };
        let top: Option<i64> = match obj.get("top") {
            Some(v) => match v.as_i64() {
                Some(0) => global_top,
                Some(n) => Some(n),
                None => {
                    let mut e = error_output("top must be an integer", "internal");
                    e["id"] = json!(obj.get("id").cloned().unwrap_or(json!(idx)));
                    batch_results.push(e);
                    continue;
                }
            },
            None => global_top,
        };
        let mut entry = match execute_query(
            client,
            endpoint,
            query,
            limit,
            fields.as_deref(),
            page,
            no_cache,
            aggregate.as_deref(),
            top,
        ) {
            Ok(v) => v,
            Err(e) => error_output(&e.message, &e.error_type),
        };
        entry["id"] = obj.get("id").cloned().unwrap_or(json!(idx));
        batch_results.push(entry);
    }
    (json!({"success": true, "results": batch_results}), 0)
}

/// Replicate argparse `parse_known_args` semantics, which clap's
/// `ignore_errors` does NOT: clap stops parsing at the first unknown
/// argument and silently drops every known flag after it (verified live:
/// `--bogus-flag --limit 1` returned 10 results, `--bogus-flag --no-cache`
/// after a cached query served stale data). So filter argv first: keep
/// known flags (and the value following a known value-flag), drop unknown
/// options and all bare tokens. The filtered argv then parses cleanly in
/// clap, whose genuine errors (e.g. non-integer `--limit`) exit 2 on
/// stderr, as argparse does.
fn filter_argv(argv: Vec<String>) -> Vec<String> {
    let value_flags: Vec<&str> = vec![
        "endpoint", "query", "limit", "fields", "page", "output", "aggregate", "top",
    ];
    let bool_flags = [
        "no-cache", "count-only", "batch", "version", "help",
    ];
    let mut out: Vec<String> = Vec::new();
    let mut iter = argv.into_iter().peekable();
    // Keep argv[0] (program name).
    if let Some(prog) = iter.next() {
        out.push(prog);
    }
    let mut expect_value = false;
    let mut after_dashdash = false;
    for tok in iter {
        if after_dashdash {
            continue; // everything after `--` is a positional extra: dropped
        }
        if expect_value {
            out.push(tok);
            expect_value = false;
            continue;
        }
        if tok == "--" {
            after_dashdash = true;
            continue;
        }
        if tok.starts_with("--") {
            let long = &tok[2..];
            let name = long.split('=').next().unwrap_or("").to_string();
            let has_eq = long.contains('=');
            if value_flags.contains(&name.as_str()) {
                out.push(tok);
                if !has_eq {
                    expect_value = true;
                }
            } else if bool_flags.contains(&name.as_str()) {
                out.push(tok);
            } else {
                // Unknown long option: dropped; a following bare token is
                // its would-be value / an extra, dropped by the bare rule.
            }
            continue;
        }
        if tok.starts_with('-') && tok.len() > 1 {
            // Unknown short option (original defines none): dropped.
            continue;
        }
        // Bare token: a positional extra, dropped (argparse puts it in
        // extras and ignores it).
    }
    out
}

fn cli() -> Command {
    Command::new("openfda-cli-rs")
        .disable_help_flag(true)
        .disable_version_flag(true)
        .arg(Arg::new("endpoint").long("endpoint").value_name("ENDPOINT").default_value(""))
        .arg(Arg::new("query").long("query").value_name("QUERY"))
        .arg(Arg::new("limit").long("limit").value_parser(clap::value_parser!(i64)).default_value("10"))
        .arg(Arg::new("fields").long("fields").value_name("FIELDS"))
        .arg(Arg::new("page").long("page").value_parser(clap::value_parser!(i64)).default_value("1"))
        .arg(Arg::new("no-cache").long("no-cache").action(ArgAction::SetTrue))
        .arg(Arg::new("count-only").long("count-only").action(ArgAction::SetTrue))
        .arg(Arg::new("output").long("output").value_name("OUTPUT"))
        .arg(Arg::new("aggregate").long("aggregate").value_name("AGGREGATE"))
        .arg(Arg::new("top").long("top").value_parser(clap::value_parser!(i64)))
        .arg(Arg::new("batch").long("batch").action(ArgAction::SetTrue))
        .arg(Arg::new("version").long("version").action(ArgAction::SetTrue))
        .arg(Arg::new("help").long("help").action(ArgAction::SetTrue))
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let matches = cli().get_matches_from(filter_argv(argv));
    let get_str = |name: &str| matches.get_one::<String>(name).map(|s| s.as_str());
    let get_i64 = |name: &str| matches.get_one::<i64>(name).copied();
    let get_flag = |name: &str| matches.get_flag(name);

    // --version wins over --help, as in the original.
    if get_flag("version") {
        output(&json!({"version": VERSION, "success": true}));
        std::process::exit(0);
    }
    if get_flag("help") {
        output(&json!({
            "usage": "openfda-cli-rs --endpoint <endpoint> [options]",
            "description": "Query the OpenFDA API. Outputs JSON only.",
            "options": {
                "--endpoint": "FDA API endpoint (e.g. drug/event, drug/label, device/event)",
                "--query": "Search query string (e.g. 'patient.drug.openfda.brand_name:aspirin')",
                "--limit": "Number of results (1-100, default 10)",
                "--fields": "Comma-separated field paths to include in output",
                "--page": "Page number for pagination (default 1)",
                "--no-cache": "Bypass cache and fetch fresh data",
                "--count-only": "Return only the total result count",
                "--output": "Comma-separated key paths to include (e.g. 'data' or 'data,meta.results.total')",
                "--aggregate": "Field path to aggregate on (counts distinct values)",
                "--top": "Number of top aggregated values to return (default: all)",
                "--batch": "Read JSON array of queries from stdin. Each query has: endpoint (required), query, limit, fields, page, no-cache, aggregate, top, id. Example: '[{\"endpoint\":\"drug/event\",\"query\":\"...\",\"limit\":50,\"aggregate\":\"patient.reaction.reactionmeddrapt\",\"top\":15}]'",
                "--version": "Print version and exit"
            },
            "success": true
        }));
        std::process::exit(0);
    }

    let logger = Logger::new();

    // Batch mode (endpoint not required here).
    if get_flag("batch") {
        let stdin_content = read_stdin_content();
        if stdin_content.is_empty() {
            output(&error_output("--batch requires JSON array on stdin", "validation"));
            std::process::exit(1);
        }
        let client = match make_client(&logger) {
            Ok(c) => c,
            Err(e) => {
                output(&error_output(&e.message, &e.error_type));
                std::process::exit(1);
            }
        };
        let (result, code) = run_batch(
            &client,
            &stdin_content,
            get_str("aggregate"),
            get_i64("top"),
            get_flag("no-cache"),
        );
        output(&result);
        if let Some(results) = result.get("results").and_then(|r| r.as_array()) {
            logger.log("INFO", &format!("Batch executed: {} queries", results.len()));
        }
        std::process::exit(code);
    }

    // Single-query mode.
    let endpoint = get_str("endpoint").unwrap_or("");
    if endpoint.is_empty() {
        output(&error_output("--endpoint is required", "validation"));
        std::process::exit(1);
    }
    let page = get_i64("page").unwrap_or(1);
    if page < 1 {
        output(&error_output("--page must be >= 1", "validation"));
        std::process::exit(1);
    }
    let query = match get_str("query") {
        Some(q) => q.to_string(),
        None => read_stdin_content(),
    };
    let limit = get_i64("limit").unwrap_or(10);
    let fields = parse_fields(get_str("fields"));

    let client = match make_client(&logger) {
        Ok(c) => c,
        Err(e) => {
            output(&error_output(&e.message, &e.error_type));
            std::process::exit(1);
        }
    };
    let data = match client.get_fda_data(endpoint, &query, limit, fields.as_deref(), page, !get_flag("no-cache")) {
        Ok(d) => d,
        Err(e) => {
            output(&error_output(&e.message, &e.error_type));
            logger.log("ERROR", &format!("Error: endpoint={} error_type={} message={}", endpoint, e.error_type, e.message));
            std::process::exit(1);
        }
    };

    if get_flag("count-only") {
        let results_len = data.get("results").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0);
        let total = match data.get("meta").and_then(|m| m.get("results")) {
            Some(Value::Object(o)) => o
                .get("total")
                .and_then(|t| t.as_i64())
                .unwrap_or(results_len as i64),
            _ => results_len as i64,
        };
        output(&json!({"success": true, "total": total}));
        logger.log("INFO", &format!("Count-only: endpoint={} total={}", endpoint, total));
        std::process::exit(0);
    }

    let results: Vec<Value> = data
        .get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let meta = data.get("meta").cloned().unwrap_or(Value::Null);
    let mut result = Map::new();
    result.insert("success".to_string(), Value::Bool(true));
    result.insert("data".to_string(), Value::Array(results.clone()));
    result.insert("meta".to_string(), meta);
    if let Some(agg) = get_str("aggregate") {
        if !agg.is_empty() {
            let agg_vals = aggregate_field(&results, agg, get_i64("top"));
            result.insert("aggregation".to_string(), json!({"field": agg, "top": agg_vals}));
        }
    }
    let mut result = Value::Object(result);
    if let Some(spec) = get_str("output") {
        if !spec.is_empty() {
            result = filter_output(&result, spec);
        }
    }
    output(&result);
    let total = match data.get("meta").and_then(|m| m.get("results")) {
        Some(Value::Object(o)) => o.get("total").and_then(|t| t.as_i64()).unwrap_or(0),
        _ => 0,
    };
    logger.log(
        "INFO",
        &format!(
            "Success: endpoint={} query={:?} limit={} page={} results={} total={}",
            endpoint,
            query,
            limit,
            page,
            results.len(),
            total
        ),
    );
    std::process::exit(0);
}
