# openfda-cli-rs

AI-native OpenFDA CLI in Rust — JSON-only output for autonomous agents.

**This is a Rust port of [lesterppo/openfda-cli](https://github.com/lesterppo/openfda-cli)
(Python, v0.1.0). It is a NEW repository; the original is untouched.**
The port keeps the original's agent-friendly contract: stdout is JSON only,
every command/flag works the same way, and errors carry the same
`error_type` classification.

## Why Rust

- **Single binary, no Python venv.** `cargo build --release` produces one
  `openfda-cli-rs` binary; no interpreter, no `pip install`, no dependency
  drift on the target machine.
- **Typed JSON handling.** Responses are parsed into `serde_json::Value`
  and projected/aggregated with the same dotted-path rules as the original,
  with the traversal logic unit-tested.
- **Same cache, real SQLite.** The cache is SQLite via `rusqlite` with a
  bundled SQLite build, so no system `libsqlite3` is needed.

## Install

```bash
# From source (recommended)
git clone https://github.com/lesterppo/openfda-cli-rs.git
cd openfda-cli-rs
cargo build --release
# binary: ./target/release/openfda-cli-rs

# Or directly with cargo
cargo install --git https://github.com/lesterppo/openfda-cli-rs.git
```

Requires a Rust toolchain (developed with rustc/cargo 1.99.1).
Builds fetch crates from crates.io; on a proxied network the shell's
normal proxy environment variables are honoured by cargo. TLS uses the
system certificate bundle (`rustls-tls-native-roots`), so a
TLS-intercepting egress proxy whose CA is installed system-wide works
as it does for curl; a WebPKI-only build would reject it.

## Usage

```bash
openfda-cli-rs --endpoint drug/event --limit 5
openfda-cli-rs --endpoint drug/label --query "openfda.generic_name:semaglutide" --fields "indications_and_usage,dosage_and_administration" --limit 2
echo "semaglutide" | openfda-cli-rs --endpoint drug/event --aggregate patient.reaction.reactionmeddrapt --top 15
```

### Endpoints

All 26 openFDA endpoints are supported — `--endpoint` passes through to
the API, exactly as in the original.

| Category | Endpoints |
|---|---|
| **Drug** (6) | `drug/event`, `drug/label`, `drug/enforcement`, `drug/ndc`, `drug/drugsfda`, `drug/shortages` |
| **Device** (9) | `device/event`, `device/recall`, `device/enforcement`, `device/510k`, `device/pma`, `device/classification`, `device/registrationlisting`, `device/udi`, `device/covid19serology` |
| **Food** (2) | `food/enforcement`, `food/event` |
| **Tobacco** (4) | `tobacco/problem`, `tobacco/researchpreventionads`, `tobacco/researchdigitalads`, `tobacco/researchsmokefree` |
| **Other** (4) | `other/nsde`, `other/substance`, `other/unii`, `other/historicaldocument` |
| **Animal & Vet** (1) | `animalandveterinary/event` |

### Flags

| Flag | Purpose | Status |
|---|---|---|
| `--endpoint` | API endpoint (required in single-query mode) | Implemented |
| `--query` | OpenFDA search query string (or piped via stdin) | Implemented |
| `--limit` | Results per page (1–100, default 10) | Implemented |
| `--fields` | Comma-separated field paths for projection | Implemented |
| `--page` | Page number (default 1; page > 1 bypasses cache) | Implemented |
| `--no-cache` | Bypass cache reads (see cache note below) | Implemented |
| `--count-only` | Return only the total result count | Implemented |
| `--output` | Filter response to specific key paths (`--output` in this port; see note) | Implemented |
| `--aggregate` | Field path to aggregate distinct values | Implemented |
| `--top` | Limit aggregation to top N | Implemented |
| `--batch` | Read JSON array of queries from stdin | Implemented |
| `--version` | Print version as JSON | Implemented |
| `--help` | Print help as JSON | Implemented |

### Output contract

Single query, success:

```json
{"success": true, "data": [...], "meta": {...}}
```

With aggregation: an extra `"aggregation": {"field": "...", "top": [{"value": "...", "count": N}]}` key.
With `--count-only`: `{"success": true, "total": N}` only (aggregation
and `--output` are ignored in this mode, as in the original).
With `--output "data,meta.results.total"`: only `success` plus the named
paths are kept; a path that cannot be walked (missing key, or a step
through a list) is silently skipped.

Errors (single-query mode, exit code 1):

```json
{"success": false, "error": "...", "error_type": "validation|network|rate_limit|not_found|api_error|internal"}
```

| `error_type` | Meaning |
|---|---|
| `validation` | Bad parameters (`--endpoint` missing, bad format, `--limit` outside 1–100, `--page` < 1, bad batch JSON) |
| `network` | Timeout, connection failure, or HTTP 5xx |
| `rate_limit` | HTTP 429 |
| `not_found` | HTTP 404 |
| `api_error` | Other HTTP error status |
| `internal` | Unexpected error |

### Batch mode

```bash
echo '[
  {"id":"adverse","endpoint":"drug/event",
   "query":"patient.drug.openfda.generic_name:semaglutide",
   "limit":50,"aggregate":"patient.reaction.reactionmeddrapt","top":15},
  {"id":"label","endpoint":"drug/label",
   "query":"openfda.generic_name:semaglutide",
   "limit":2,"fields":"openfda.brand_name,indications_and_usage,dosage_and_administration,boxed_warning"}
]' | openfda-cli-rs --batch --no-cache
```

Each entry gets `id` (its own `id`, else its index), keeps error
isolation (one failing query does not abort the batch), and may override
`aggregate`, `top`, `no-cache`, `fields`, `page`, `limit` per query.
`--output` and `--count-only` do not apply in batch mode, as in the
original.

### Caching

SQLite cache with 24-hour TTL. Cache key is
`{endpoint}|{query}|{limit}` (raw query, before sanitising; `page` and
`fields` are NOT part of the key). Only page 1 is cached; field
projection is applied after cache retrieval, so different `--fields`
values share cache entries. `--no-cache` bypasses cache *reads* but a
fresh page-1 fetch is still *written* to the cache — this quirk of the
original is replicated exactly.

Environment variables:

| Variable | Default | Purpose |
|---|---|---|
| `OPENFDA_API_BASE` | `https://api.fda.gov` | API base URL (same as original) |
| `OPENFDA_CACHE_PATH` | `./fda_cache.db` | Cache file location |

Logs are appended to `./logs/fda_agent.log` (best-effort; a logging
failure never breaks stdout).

## Parity table (original Python vs this port)

Honest status after the live deep test of 2026-10-26 (see
[DEEP_TEST.md](DEEP_TEST.md) for the full numbers). "Implemented" means
verified by `cargo test` and/or the live comparison, not just written.

| Command / flag | Status | Notes |
|---|---|---|
| `--endpoint` (all 26 endpoints) | Implemented | Pass-through to `api.fda.gov`; `.json` suffix appended unless present |
| `--query` (incl. stdin pipe) | Implemented | Control chars stripped, trimmed, same as original |
| `--limit` (1–100) | Implemented | Out-of-range → `validation` error |
| `--fields` projection (dotted paths, list traversal, numeric index, dedupe) | Implemented | Same traversal rules, incl. multi-value dedupe |
| `--page` / pagination (`skip = (page-1)*limit`) | Implemented | Page < 1 → `validation` in single mode; page > 1 bypasses cache |
| `--no-cache` | Implemented | Bypasses reads; page-1 fetch still writes cache (original quirk) |
| `--count-only` | Implemented | Returns `{"success": true, "total": N}` only |
| `--output` dot-path filter | Implemented | Nested paths recreate intermediate objects; missing paths skipped |
| `--aggregate` + `--top` | Implemented | String values only, count descending, ties in first-seen order |
| `--batch` (incl. per-query overrides, error isolation) | Implemented | Batch JSON errors exit 0 with `success: false` (original quirk) |
| `--version` / `--help` (JSON) | Implemented | `--version` wins over `--help`, as in the original |
| Error classification (`rate_limit` / `not_found` / `network` / `api_error` / `validation` / `internal`) | Implemented | See differences below for `api_error` message wording |
| SQLite cache (24h TTL, page-1 only) | Implemented | Via `rusqlite` with bundled SQLite |
| File logging (`logs/fda_agent.log`, 12-hex session id) | Implemented | Format equivalent; timestamp is local time |
| `client.batch_execute()` (Python helper method) | Not exposed | The original CLI never calls it either; batch mode is implemented via the CLI path |

## Deliberate differences from the original

1. **Cache/log location.** The original resolves `fda_cache.db` and
   `logs/` relative to its own source tree. An installed binary has no
   source tree, so this port uses the working directory instead
   (`OPENFDA_CACHE_PATH` overrides the cache path).
2. **Retry actually works.** The original decorates its fetch with a
   tenacity retry for `NetworkError`/`RateLimitError`, but the fetch only
   ever raises raw httpx errors, so the retry never fires. This port
   retries network errors, 429s and 5xx up to 3 attempts (2s, then 4s
   backoff) — the policy the original configured but never executed.
3. **`api_error` message wording.** The original's message embeds the
   httpx exception text (`HTTP 400: Client error ... for url ...`).
   This port reports `HTTP <status>` for non-429/404/5xx statuses.
   The `error_type` is identical; only the message text differs.
4. **Help `usage` string.** Says `openfda-cli-rs` instead of
   `openfda-cli`; all option keys and descriptions are unchanged.
5. **Argument parsing.** The original uses `argparse` with
   `parse_known_args` (unknown arguments ignored, bad `--limit` exits 2
   with a usage error on stderr). This port pre-filters argv to the same
   semantics and then uses `clap` — unknown options and bare tokens are
   dropped, but known flags after an unknown one are still parsed (an
   early build used clap's `ignore_errors`, which dropped everything
   after the first unknown flag; the deep test caught this and it was
   fixed — see [DEEP_TEST.md](DEEP_TEST.md) §4a). A non-integer
   `--limit`/`--page`/`--top` is rejected by clap (exit 2, stderr)
   rather than producing JSON.
6. **Cache-open failure.** If the cache file cannot be opened, this port
   logs the failure and continues cacheless instead of failing the query.
7. **Dedupe key for projected multi-values.** The original dedupes by
   Python `str(value)` (so integer `1` and string `"1"` collide); this
   port dedupes by JSON text (they are distinct). No real API payload
   observed in testing hit this edge.
8. **No LICENSE file in the ported snapshot.** The `/tmp` snapshot of
   the original used for this port contains no LICENSE file; this repo
   ships the MIT text per the porting brief (same licence family as the
   original project).

## For AI agents

See [`skills/openfda-cli.md`](skills/openfda-cli.md) — the original skill
file, adapted to this binary's name and install method, with field paths
per endpoint, search syntax, error types, and batch report patterns.

## Development

```bash
cargo test            # unit tests: sanitising, traversal, projection,
                      # aggregation, --output filtering, error
                      # classification, cache round-trip/expiry
cargo build --release # ./target/release/openfda-cli-rs
```

## License

MIT — see [LICENSE](LICENSE).
