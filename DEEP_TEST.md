# Deep test: openfda-cli-rs vs openfda-cli (Python original)

**Date:** 2026-10-26 (Asia/Hong_Kong). **Method:** both binaries run
back-to-back in the same session against live `api.fda.gov`, same inputs,
outputs compared as parsed JSON (exact equality = same keys, same values,
same nesting). The Python original was run from a scratch copy
(`/tmp/openfda-py-test`, venv with httpx/pydantic/tenacity); the original
tree `/tmp/openfda-cli` was left untouched (`git status` clean afterwards).
The Rust binary tested is the release build in this repo
(`cargo build --release`, 7,870,584 bytes).

**Verdict: PASS with two wording-only divergences and one real bug found
and fixed during testing** (see §4). 22 of 24 comparison cases are
byte-for-byte identical as parsed JSON; the other 2 differ only in
human-readable message text, with identical keys, exit codes and
`error_type`.

## 1. Comparison table (final run, fixed binary)

| Case | Rust / Python exit | Keys equal | Exact equal | Totals (RS / PY) | Notes |
|---|---|---|---|---|---|
| `drug/label` aspirin, limit 2 | 0 / 0 | yes | **yes** | 677 / 677 | n=2 both; first record fields identical |
| Same + `--fields openfda.brand_name,openfda.generic_name` | 0 / 0 | yes | **yes** | 677 / 677 | Projection identical |
| Same + `--count-only` | 0 / 0 | yes | **yes** | `{"total": 677}` both | |
| `device/event`, limit 2 (no query) | 0 / 0 | yes | **yes** | 26,557,775 / 26,557,775 | n=2 both |
| `food/enforcement`, limit 2 | 0 / 0 | yes | **yes** | 29,505 / 29,505 | n=2 both |
| `other/substance`, limit 2 | 0 / 0 | yes | **yes** | 167,385 / 167,385 | n=2 both |
| Pagination page 1, aspirin limit 2 | 0 / 0 | yes | **yes** | 677 / 677 | `meta.results.skip` = 0 |
| Pagination page 2, aspirin limit 2 | 0 / 0 | yes | **yes** | 677 / 677 | `skip` = 2; page1 ∩ page2 = ∅ in both |
| `--output data,meta.results.total` | 0 / 0 | yes | **yes** | 677 / 677 | Filter spec identical |
| `--output meta.results.total` (nested only) | 0 / 0 | yes | **yes** | 677 / 677 | Intermediate objects recreated |
| `--output data,meta.nope.deep` (missing path) | 0 / 0 | yes | **yes** | — | Missing path silently skipped, both |
| `drug/event` semaglutide + `--aggregate patient.reaction.reactionmeddrapt --top 5` | 0 / 0 | yes | **yes** | 73,001 / 73,001 | Aggregation identical: Asthma 6, Dyspnoea 5, Inappropriate schedule of product administration 4, Wheezing 4, Blood pressure increased 4 |
| Piped stdin query (`echo aspirin \| … drug/label`, limit 2) | 0 / 0 | yes | **yes** | 27,228 / 27,228 | Free-text search, both |
| Batch (label + event-aggregate, ids `label`/`events`) | 0 / 0 | yes | **yes** | — | Entries identical, ids preserved |
| Batch with invalid JSON | 0 / 0 | yes | no — wording only | — | Both `success:false`, `error_type:validation`, exit 0 (original quirk); message text differs: serde vs Python json text |
| Nonsense query (`openfda.generic_name:zzzznotadrug123`) | 1 / 1 | yes | **yes** | — | Both `not_found`, message `FDA endpoint returned 404` |
| Bad endpoint (`drug/bogus`) | 1 / 1 | yes | **yes** | — | Both `not_found`, same message |
| `--limit 0` | 1 / 1 | yes | **yes** | — | Both `validation`, `Limit must be between 1 and 100` |
| `--limit 200` | 1 / 1 | yes | **yes** | — | Same as above |
| No `--endpoint` | 1 / 1 | yes | **yes** | — | Both `validation`, `--endpoint is required` |
| `--page 0` | 1 / 1 | yes | **yes** | — | Both `validation`, `--page must be >= 1` |
| Unknown flag (`--bogus-flag`, before `--limit 1 --no-cache`) | 0 / 0 | yes | **yes** | 677 / 677 | n=1 both after fix (§4) |
| `--version` | 0 / 0 | yes | **yes** | — | `{"version":"0.1.0","success":true}` both |
| `--help` | 0 / 0 | yes | no — wording only | — | Only the `usage` string differs: `openfda-cli-rs` vs `openfda-cli` (deliberate rename); all option keys/descriptions identical |

No field was dropped, reordered, or added by the Rust version in any
exact-equal case: comparisons are on parsed JSON values, including full
record bodies (e.g. the aspirin label records, ~15 KB each).

## 2. Cache test (dead-API proof)

Cache isolation: Rust used its own cache file, Python its copy's
`fda_cache.db`. Sequence per binary, query = aspirin label, limit 2:

| Step | Rust | Python |
|---|---|---|
| (a) Fresh fetch | ok, total 677, 0.76 s | ok, total 677, 1.68 s |
| (b) Same query, `OPENFDA_API_BASE=http://127.0.0.1:9` (dead) | **ok, byte-identical data, 0.01 s** → cache hit proven | **ok, byte-identical data, 0.52 s** → cache hit proven |
| (c) Dead API + `--no-cache` | `network` error, 6.01 s (retry backoff, §5) | `network` error, 0.47 s |
| (d) Dead API, page 2 | `network` error → page > 1 not cached | `network` error → page > 1 not cached |
| (e) Dead API + `--fields openfda.brand_name` | ok from cache, data keys exactly `["openfda.brand_name"]` → projection after retrieval | Same |

## 3. Performance (timed, not claimed beyond measurement)

- Cold start (`--version`, 5 runs each): Rust min 0.003 s, median
  0.003 s, mean 0.004 s; Python min 0.336 s, median 0.344 s, mean
  0.358 s. Python pays interpreter + httpx/pydantic import on every run.
- Release binary: 7,870,584 bytes (~7.5 MiB), single file, no runtime.
  Python has no binary; its source tree is 96 KB plus interpreter and
  installed packages.
- Live-query wall times in §1 (network-bound, same proxy): Rust was
  typically 0.7–0.9 s vs Python 1.2–1.9 s per label query, and 3.35 s vs
  5.71 s for `device/event`. These are single-session measurements, not
  a benchmark.

## 4. Divergences found during testing

### 4a. FIXED — clap dropped flags after an unknown flag (real bug)

First deep-test run used clap's `ignore_errors(true)`. It turned out
clap stops parsing at the first unknown argument: `--bogus-flag --limit
1` returned 10 results (limit dropped) and `--bogus-flag --no-cache`
served stale cached data (flag dropped), while Python's
`parse_known_args` parses known flags after unknown ones. Proven with a
dead API: `--bogus-flag --no-cache` succeeded from cache in the old
build. **Fix:** `main.rs` now pre-filters argv to `parse_known_args`
semantics (known flags kept, unknown options and bare tokens dropped)
and feeds the filtered argv to clap without `ignore_errors`. After the
fix: dead-API `--bogus-flag --no-cache` correctly fails with `network`,
`--bogus-flag --limit 1` returns n=1, and a non-integer `--limit`
exits 2 with a clap error on stderr (as argparse does) instead of
panicking on a partial match. The final §1 run uses the fixed binary.

### 4b. Wording only — `--help` usage string

`openfda-cli-rs` vs `openfda-cli`. Deliberate (new binary name).

### 4c. Wording only — batch JSON parse error text

Rust (serde_json): `Invalid batch JSON: expected ident at line 1
column 2`. Python: `Invalid batch JSON: Expecting value: line 1 column
1 (char 0)`. Same `error_type` (`validation`), same exit code (0, the
original's batch quirk).

### 4d. Not triggered live — rate limiting

HTTP 429 was not triggered (hammering the API to force one would be
abusive and was not attempted). The `rate_limit` classification is
unit-tested (`classify_status(429)`) and shares the code path verified
for 404/5xx/network. The 5xx/`network` path was verified live via the
dead-API tests above.

## 5. Behavioural differences vs the original (all deliberate, in README)

- **Retry actually works in Rust** (2 s, then 4 s backoff, 3 attempts
  for network/429/5xx). The original configures the same tenacity retry
  but it never fires (raw httpx errors are not the retried types), so
  Python fails fast (0.47 s) where Rust takes ~6 s on a dead host.
  Visible in §2 step (c)/(d).
- **Cache/log location**: working directory (`OPENFDA_CACHE_PATH`
  overrides) instead of the original's source-tree-relative paths.
- **`api_error` message**: `HTTP <status>` instead of httpx's exception
  text; `error_type` identical.
- **Dedupe key** for projected multi-values: JSON text instead of
  Python `str()` (integer `1` vs string `"1"` no longer collide).
  No API payload in this test hit that edge.

## 6. Test-environment quirks (not port divergences)

- **TLS-intercepting egress proxy.** The VM proxy re-signs TLS with a
  CA in the system bundle. Curl trusts it; Rust with WebPKI roots did
  not (bare `Connection failed`). Fixed by building reqwest with
  `rustls-tls-native-roots`. Recorded in `Cargo.toml`.
- **Bracketed IPv6 in `NO_PROXY`.** The VM default
  `NO_PROXY=localhost,127.0.0.1,::1,[::1],198.19.0.1` makes httpx crash
  at client construction (`Invalid port: ':1]'`). Both runners used a
  cleaned `NO_PROXY=localhost,127.0.0.1` for the Python side; this is a
  pre-existing httpx quirk, not something the port introduced.
- **Stdin inheritance (harness artifact, found twice).** Cases with no
  `--query` read stdin when it is not a terminal. The first harness
  runs passed an open pipe as stdin, so both binaries correctly blocked
  waiting for a piped query and the runs timed out (first misread as a
  `device/event` hang in Rust, then the same stall appeared in Python).
  The harness was fixed to pass a closed (EOF) stdin when no piped
  input is intended. `device/event` itself runs in 3–5 s.

## 7. What was NOT verified

- `rate_limit` (429) live — see §4d.
- All 26 endpoints live — 4 categories exercised live (drug, device,
  food, other); the remaining endpoints share the identical
  pass-through code path (`--endpoint` is not validated against a list
  in either implementation), but their individual response shapes were
  not fetched.
- `client.batch_execute()` — a Python helper the original CLI never
  calls; not exposed in the Rust port (documented in README).
