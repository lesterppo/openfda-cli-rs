---
name: openfda-cli-rs
description: Use when querying FDA data (adverse events, drug labels, NDC, approvals, enforcement, shortages, devices, food) via openfda-cli-rs. Triggered by: FDA, OpenFDA, GLP1, LAMA, drug safety, FAERS, adverse events, drug labeling, drug dosage, indications, side effects, drug studies, NDC, drug approval, drug shortages, medical data analysis.
---

# openfda-cli-rs — AI-Native OpenFDA CLI

A single-binary CLI for querying the OpenFDA API. Outputs ONLY clean JSON — no human-readable text to stdout.

## Install

```bash
cargo install --git https://github.com/lesterppo/openfda-cli-rs.git
# Then: openfda-cli-rs --endpoint ...
```

## Quick Start

```bash
openfda-cli-rs --endpoint drug/event --limit 5
openfda-cli-rs --endpoint drug/event --fields patient.reaction.reactionmeddrapt --limit 10
echo "aspirin" | openfda-cli-rs --endpoint drug/event --limit 3
```

### Dose & Indication Lookup

```bash
# Dosage for a specific drug
openfda-cli-rs --endpoint drug/label \
  --query "openfda.generic_name:semaglutide" \
  --fields "openfda.brand_name,dosage_and_administration,indications_and_usage" \
  --limit 2

# Drug approval history (studies, application data)
openfda-cli-rs --endpoint drug/drugsfda \
  --query "openfda.generic_name:semaglutide" \
  --limit 3

# Current shortages for a drug class
openfda-cli-rs --endpoint drug/shortages \
  --query "openfda.generic_name:lisdexamfetamine" \
  --fields "presentation,status,shortage_reason,availability"
```

## Available Endpoints

### Drug API

| Endpoint | Data | Example Query |
|---|---|---|
| `drug/event` | FAERS adverse event reports (~1.8M records) | `patient.drug.openfda.generic_name:semaglutide` |
| `drug/label` | Structured product labeling (prescribing info) | `openfda.generic_name:semaglutide` |
| `drug/enforcement` | Drug product recall enforcement reports | `tiotropium` |
| `drug/ndc` | National Drug Code directory (~135K records) | `generic_name:metformin` |
| `drug/drugsfda` | Drugs@FDA — approvals since 1939 (~29K apps) | `openfda.brand_name:ozempic` |
| `drug/shortages` | Current drug shortage reports (~1.7K) | `openfda.generic_name:lisdexamfetamine` |

### Device API

| Endpoint | Data |
|---|---|
| `device/event` | Medical device adverse events (MAUDE) |
| `device/recall` | Device recall enforcement |
| `device/enforcement` | Device enforcement actions |
| `device/510k` | 510(k) premarket notifications |
| `device/pma` | Premarket approvals |
| `device/classification` | Device classification |
| `device/registrationlisting` | Device registrations and listings |
| `device/udi` | Unique Device Identifier data |
| `device/covid19serology` | COVID-19 serology test data |

### Food API

| Endpoint | Data |
|---|---|
| `food/enforcement` | Food recall enforcement reports |
| `food/event` | Food, dietary supplement, cosmetic adverse events |

### Tobacco API

| Endpoint | Data |
|---|---|
| `tobacco/problem` | Tobacco problem reports |
| `tobacco/researchpreventionads` | Tobacco prevention ads research |
| `tobacco/researchdigitalads` | Tobacco digital ads research |
| `tobacco/researchsmokefree` | Tobacco smokefree research |

### Other API

| Endpoint | Data |
|---|---|
| `other/nsde` | National Substance Data Engine (~658K) |
| `other/substance` | Substance data (~167K) |
| `other/unii` | Unique Ingredient Identifier lookup |
| `other/historicaldocument` | Historical FDA documents |

### Animal & Veterinary API

| Endpoint | Data |
|---|---|
| `animalandveterinary/event` | Animal/veterinary adverse event reports |

All endpoints accept the same query syntax and flags. Use `--endpoint <path>` with any of the above.

## Search Syntax

- **By generic name:** `patient.drug.openfda.generic_name:semaglutide`
- **By brand name (label):** `openfda.brand_name:SPIRIVA`
- **By brand name (events):** `patient.drug.openfda.brand_name:Ozempic`
- **Combined:** `patient.drug.openfda.generic_name:semaglutide+AND+serious:1`
- **Piped queries:** `echo "aspirin" | openfda-cli-rs --endpoint drug/event --limit 3`

## Key Flags

| Flag | Purpose |
|---|---|
| `--endpoint` | API endpoint (required for single-query mode) |
| `--query` | Search string. Can also be piped via stdin. |
| `--limit` | Results per page (1-100, default 10) |
| `--fields` | Comma-separated field paths for token-efficient projection. Use dotted notation for nested fields (e.g., `patient.reaction.reactionmeddrapt,safetyreportid`) |
| `--page` | Page number for pagination (page > 1 bypasses cache) |
| `--no-cache` | Bypass SQLite cache |
| `--count-only` | Return just `{"total": N}` — minimal response, useful for volume checks |
| `--output` | Filter response to specific key paths. Example: `--output "data,meta.results.total"` strips the disclaimer/license boilerplate |
| `--aggregate` | Field path to aggregate on. Counts distinct string values across results. Example: `--aggregate patient.reaction.reactionmeddrapt` |
| `--top` | Limit aggregation to top N. Use with `--aggregate`. Example: `--aggregate ... --top 15` |

## Batch Mode — Multi-Query Pipelines

The most powerful feature for report generation. Reads a JSON array of queries from stdin, executes each with error isolation, and returns combined results. **Use `--no-cache` to ensure fresh data across all queries.**

```bash
echo '[
  {"id":"semaglutide_events", "endpoint":"drug/event",
   "query":"patient.drug.openfda.generic_name:semaglutide",
   "limit":50, "aggregate":"patient.reaction.reactionmeddrapt", "top":15},
  {"id":"tirzepatide_events", "endpoint":"drug/event",
   "query":"patient.drug.openfda.generic_name:tirzepatide",
   "limit":50, "aggregate":"patient.reaction.reactionmeddrapt", "top":15},
  {"id":"semaglutide_label", "endpoint":"drug/label",
   "query":"openfda.generic_name:semaglutide",
   "limit":2, "fields":"openfda.brand_name,indications_and_usage,warnings_and_cautions,dosage_and_administration"},
  {"id":"tirzepatide_label", "endpoint":"drug/label",
   "query":"openfda.generic_name:tirzepatide",
   "limit":2, "fields":"openfda.brand_name,indications_and_usage,warnings_and_cautions,dosage_and_administration"},
  {"id":"semaglutide_approval", "endpoint":"drug/drugsfda",
   "query":"openfda.generic_name:semaglutide",
   "limit":5, "fields":"application_number,sponsor_name,products.brand_name,products.dosage_form,products.active_ingredients,submissions.submission_status_date"}
]' | openfda-cli-rs --batch --no-cache
```

Each query can override defaults with its own `aggregate`, `top`, `no-cache`, `fields`, `page`, and `limit`.

### Batch Output Structure

```json
{"success": true, "results": [
  {"id": 0, "success": true, "data": [...], "meta": {...}, "aggregation": {...}},
  {"id": 1, "success": false, "error": "FDA endpoint returned 404", "error_type": "not_found"},
  {"id": 2, "success": true, "data": [...], ...}
]}
```

- Successful results have: `success: true`, `data` (list of results), `meta`, `aggregation` (if aggregate was requested)
- Failed results have: `success: false`, `error`, `error_type`
- One query failure does NOT abort the batch — error is isolated per query

## Report Generation Pattern

For multi-source medical reports (drug class profiles), follow this sequence:

1. **Volume assessment** — batch `--count-only` queries across all drugs to assess data availability
2. **FAERS aggregation** — batch `--aggregate` queries with `--top 15` for adverse reaction profiling
3. **Labeling** — batch label queries for indications, dosing, warnings, boxed warnings
4. **Approval history** — batch `drug/drugsfda` queries for approval dates, application types, sponsors
5. **Enforcement** — batch `drug/enforcement` queries for recall history
6. **Shortages** — (optional) batch `drug/shortages` queries for current supply status
7. **Compile** — structure into markdown with: overview → dosing/indications → warnings → FAERS profile → approval history → enforcement → shortages → caveats

## Common Field Paths

### drug/event (FAERS)

| Path | Description |
|---|---|
| `safetyreportid` | Unique report identifier |
| `patient.reaction.reactionmeddrapt` | Adverse reaction terms (MedDRA) |
| `patient.drug.medicinalproduct` | Drug product names from report |
| `patient.drug.openfda.generic_name` | Standardized generic name |
| `patient.drug.openfda.brand_name` | Standardized brand name |
| `serious` | Seriousness flag (1=serious, 2=non-serious) |

### drug/label (Product Labeling)

| Path | Description |
|---|---|
| `openfda.brand_name` | Brand name |
| `openfda.generic_name` | Generic name |
| `openfda.route` | Route of administration |
| `indications_and_usage` | Approved indications |
| `dosage_and_administration` | Dosing information |
| `dosage_forms_and_strengths` | Available strengths/forms |
| `boxed_warning` | Boxed warning text |
| `warnings_and_cautions` | Warnings & precautions |
| `adverse_reactions` | Adverse reactions from label |
| `contraindications` | Contraindications |

### drug/drugsfda (Drugs@FDA)

| Path | Description |
|---|---|
| `application_number` | NDA/ANDA/BLA number |
| `sponsor_name` | Sponsor/applicant name |
| `products.brand_name` | Brand name |
| `products.dosage_form` | Dosage form (tablet, injection, etc.) |
| `products.route` | Route of administration |
| `products.marketing_status` | Prescription, OTC, Discontinued |
| `products.active_ingredients.name` | Active ingredient name |
| `products.active_ingredients.strength` | Ingredient strength |
| `submissions.submission_type` | Submission type (ORIG, SUPPL, etc.) |
| `submissions.submission_status` | Status (AP=approved, TA=tentative) |
| `submissions.submission_status_date` | Approval date |
| `openfda.brand_name` | Harmonized brand name |
| `openfda.generic_name` | Harmonized generic name |

### drug/ndc (NDC Directory)

| Path | Description |
|---|---|
| `product_ndc` | Product-level NDC code |
| `brand_name_base` | Base brand name |
| `generic_name` | Generic/product name |
| `labeler_name` | Labeler/manufacturer name |
| `active_ingredients.name` | Active ingredient name |
| `active_ingredients.strength` | Ingredient strength |
| `dosage_form` | Dosage form |
| `product_type` | HUMAN PRESCRIPTION DRUG, OTC, etc. |
| `marketing_category` | NDA, ANDA, BLA, OTC monograph |
| `marketing_start_date` | Marketing start date |
| `packaging.package_ndc` | Package-level NDC code |
| `packaging.description` | Package description |
| `openfda.brand_name` | Harmonized brand name |
| `openfda.generic_name` | Harmonized generic name |
| `openfda.route` | Route of administration |

### drug/shortages

| Path | Description |
|---|---|
| `generic_name` | Drug/product name |
| `company_name` | Manufacturer |
| `status` | "Current" or "Resolved" |
| `availability` | Availability summary |
| `shortage_reason` | Reason for shortage |
| `dosage_form` | Dosage form |
| `presentation` | Product presentation text |
| `therapeutic_category` | Therapeutic class |
| `initial_posting_date` | First reported date |
| `update_date` | Last update date |
| `openfda.generic_name` | Harmonized generic name |
| `openfda.brand_name` | Harmonized brand name |

### drug/enforcement

| Path | Description |
|---|---|
| `reason_for_recall` | Enforcement recall reason |
| `classification` | Recall class (I/II/III) |
| `status` | Enforcement status |

## Error Types

| `error_type` | Meaning |
|---|---|
| `network` | Timeout, DNS failure, 5xx |
| `rate_limit` | HTTP 429 |
| `not_found` | HTTP 404 |
| `validation` | Bad parameters |
| `internal` | Unexpected error |

## Caching

- SQLite cache with 24-hour TTL at `fda_cache.db`
- Cache key: `{endpoint}|{query}|{limit}` (page and fields are NOT part of the key)
- Field projection is applied AFTER cache retrieval, so different `--fields` values share cache entries
- `--no-cache` bypasses cache for a single query
- `--page > 1` queries never cache
