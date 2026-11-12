# 10 · Tool Output Protocol v0.1

> Language: **English** | [简体中文](10-tool-output-protocol-v0.1.zh-CN.md)

> Status: **draft (2026-11-04)**, based on four confirmed decisions:  
> 1. `items` uses short keys; the human-readable format is converted by tool code and costs no model tokens;  
> 2. the three-tier budget starts at **8KB / 24KB / 128KB**, with telemetry, tuned while in use;  
> 3. M0 **does not do MCP**; Headroom is integrated via proxy/wrap/SDK;  
> 4. `summary` is Chinese by default, switchable to English/Japanese.  
>
> Companion schema: [`schemas/tool-result.v1.json`](../schemas/tool-result.v1.json).  
> Related documents: [08-design-roadmap-v0.2.md](08-design-roadmap-v0.2.md) §6, [09-code-search-and-knowledge-base.md](09-code-search-and-knowledge-base.md) §8.

---

## 1. Goals and non-goals

### 1.1 Goals

1. **Slim down at the tool first**: what enters the model context is "summary + address + preview", not the complete result.
2. **Enforceable**: limits are enforced by wrapper code, not by the prompt's good behavior.
3. **Transport-agnostic**: the same envelope serves the CLI, the internal API, and the future native executor.
4. **Deterministic**: sorting, field order, and truncation rules are stable, which makes caching and reproduction easy.
5. **Observable**: every call can answer "how much was returned, was it truncated, did it spill, was it a duplicate".
6. **Human readability costs no tokens**: the model sees short keys; humans see `--format pretty` or `loom show`, and the conversion happens at the tool layer.

### 1.2 Non-goals

- No MCP server (M0).
- No rich text / tables / terminal UI.
- No images, PDFs, or binary content returned.
- No replacement for Headroom's cache/compression; the two are layered.
- No cross-run content-addressed cache in v1 (reserved).

---

## 2. Envelope v1

```json
{
  "v": 1,
  "ok": true,
  "tool": "search",
  "summary": "12 matches in 4 files; returned the first 8",
  "items": [
    {
      "t": "match",
      "p": "apps/web/src/auth/session.ts",
      "b": "9f3c1a2b",
      "r": [120, 128],
      "s": "Session.refresh",
      "k": "def",
      "x": "export async function refresh(...)",
      "sc": 0.87
    }
  ],
  "ref": "artifacts/search-003.txt",
  "truncated": true,
  "next": "cur_7f3a",
  "stats": { "n": 12, "bytes": 1832, "tok": 420, "spill": true }
}
```

Conventions:

- **Long keys appear only once on the outer level**; `items` uses short keys, saving tokens where they repeat.
- **Absent fields are omitted outright**, never written as `null`.
- `summary` is a line for humans; the model mainly reads `items` and `ref`.
- `text` is only for tools that need body text, such as `read` / `artifact` / `log`, and is capped by `read_bytes`; the full content still goes to `ref`.
- `stats.bytes` is the actual UTF-8 byte count of this envelope; `stats.tok` is an estimate.
- Producers must write `v: 1` and conform to the schema; consumers treat a missing `v` as v1 and ignore unknown fields (forward compatibility).
- `ref` is relative to the run directory, in the form `artifacts/search-003.txt`.

### 2.1 Item short keys

| Short key | Meaning | Applies to `t` |
|---|---|---|
| `t` | type: `match` / `file` / `dir` / `path` / `hunk` / `log` / `symbol` | all |
| `p` | repo-relative path | all except `log` |
| `b` | content hash (git blob / sha256 prefix) | `match` / `file` / `symbol` |
| `r` | 1-based closed-interval line numbers `[start, end]` | `match` / `file` / `hunk` / `log` / `symbol` |
| `s` | symbol name | `match` / `symbol` |
| `k` | kind: `def` / `ref` / `call` / `type` / `test` / `doc` / `config` / `other` | `match` / `symbol` |
| `x` | one-line preview (≤400 characters) | `match` / `log` / `hunk` |
| `sc` | relevance 0–1 | `match` / `symbol` |
| `n` | total / sub-item count / total lines | interpreted by `t` |
| `a` / `d` | added / deleted line counts | `hunk` / `file` |
| `l` | language identifier (rust/ts/py…) | `file` |

The **human view** is produced by tool code expanding short keys into long keys, tables, or colored output; this happens locally and **does not enter the model context**.

---

## 3. Limits

### 3.1 Single call

| Tool | Default return | Hard cap | Behavior over the cap |
|---|---:|---:|---|
| `search` | 20 hits / 4KB | 50 hits / 8KB | spill to artifact, return top N + `next` |
| `read` | 120 lines / 8KB | 200 lines / 12KB | must continue reading by `range` |
| `tree` | depth 2 / 100 entries | depth 3 / 200 entries | return a directory summary + `ref` |
| `glob` | 100 paths / 4KB | 200 / 8KB | truncate + `next` |
| `diff` | 300 lines / 12KB | 500 lines / 20KB | return per-file stat + `ref` |
| `verify` / `log` | last 120 lines / 8KB | 200 lines / 12KB | full log goes into `ref` |
| `git` | 20 one-liners | 50 | truncate |
| `bash` | last 120 lines / 8KB | 200 lines / 12KB | full output goes into `ref` |
| `edit` / `write` | no full text returned | — | return only path, hunk count, `a`/`d` |

Unified rules:

- `search`'s `x` defaults to 1 line, at most 160 characters; `context_lines` defaults to 0, max 3.
- A single line is at most 400 characters; truncated at UTF-8 character boundaries, never cutting Chinese/Japanese mid-character.
- Binary (contains NUL) → `err.code=binary`, no content returned.
- Files larger than 1MB must come with `range`.
- Empty results still return a `summary`, never an empty string.
- Sorting is fixed: `sc` descending → `p` lexicographic → `r[0]` ascending.
- Field order is fixed, which helps prefix caching.

### 3.2 Three-tier budget

| Tier | Default | Hard cap | After the cap |
|---|---:|---:|---|
| single call | 8KB | 16KB | spill to artifact + truncate |
| single agent invocation | 24KB | 48KB | return only `summary + ref`; `read` drops to 40 lines |
| single run (including retries) | 128KB | 256KB | force a narrower `scope`, otherwise `err.code=budget` |

- Counting basis: **the UTF-8 bytes actually written into the envelope**, excluding the full text spilled to disk.
- At 75%, a `budget` field is attached:

```json
"budget": { "u": 98304, "l": 131072, "p": 75, "at": "run" }
```

- 8KB of Chinese is about 2700 characters, with a higher token density than English; so record both `bytes` and `tok`, and do not judge by bytes alone.
- Defaults can be lowered in `.loom/config.toml`; hard caps are compiled into the code, and only an explicit admin config can raise them.

### 3.3 Input side

- `search.q` ≤ 200 characters; `path_glob` ≤ 5 entries; `max_results` over the cap is automatically clamped to the hard cap.
- `read.path` must be a repo-relative path; `..`, absolute paths, and symlink escapes are rejected.
- Repo-wide search without a `path`: allowed, but ignore rules and a result cap are applied automatically; when the task declares a `scope`, only the scope is searched by default.
- `edit` patch ≤ 200 lines; beyond that a split is required.
- All arguments are normalized before execution; **the budget cannot be left to prompt constraints**.

---

## 4. Pagination, artifacts, and dedup

### 4.1 Pagination

- `next` is an opaque cursor encoding `query_hash + offset + snapshot`.
- After the repository changes the cursor is invalidated → `err.code=stale_cursor`, a re-query is required, and misaligned results are not returned.
- A cursor is only valid within the same run; it is not reused across runs.

### 4.2 Artifact

Path: `.loom/runs/<run>/artifacts/<tool>-<seq>.txt`

The first line is a JSON header, followed by the full text:

```json
{"v":1,"tool":"search","args_hash":"ab12","git":"9f3c1a2","cwd":".","created":"2026-11-04T12:00:00Z","count":12,"sha256":"..."}
```

- `ref` exposes only the relative path; the worker reads it with `loom artifact --ref ... --range ...`.
- Artifacts are read-only; every call creates a new file and nothing is overwritten.
- Kept until the run ends; `loom gc` cleans them up in one place.
- No secrets are written into artifacts; command arguments are recorded only as `args_hash`, and only debug mode records the redacted args.

### 4.3 Dedup

- If `(p, b, r, tool)` has already been returned within the same run, return:

```json
{"v":1,"ok":true,"tool":"read","unchanged":true,"ref":"artifacts/read-002.txt","summary":"content unchanged; already returned in this run"}
```

- Content is resent only when the model explicitly sets `force`.
- Search dedup uses `query_hash + result_set_hash`; if the result set is unchanged, `unchanged` is returned.
- A changed file (`b` different) does not count as a duplicate.

---

## 5. Errors and exit codes

```json
{
  "v": 1,
  "ok": false,
  "tool": "read",
  "summary": "file too large; specify a line range",
  "err": { "code": "too_large", "hint": "read --range 1:200" }
}
```

`err.code` is a fixed enum:

`invalid_args`, `not_found`, `too_large`, `binary`, `timeout`, `permission`, `stale`, `stale_cursor`, `budget`, `internal`.

- Only one reason line is returned; `detail` ≤500 characters, stack traces forbidden.
- `summary` is localized; `code` is not translated.
- Tool CLI exit codes: `0` success, `2` bad arguments, `3` timeout/budget, `4` internal error, `5` stale.
- **The adapter's exit-code contract (0/2/3/4) stays unchanged**; the two must not be mixed up.

---

## 6. Localization and CJK

- `LOOM_LANG=zh-CN` (default) | `en` | `ja`.
- Only `summary`, `err.hint`, `err.detail`, and the human view are translated; paths, symbols, enums, and field names are not.
- Chinese first; English templates land with the features; Japanese last.
- `rg` works for exact CJK substrings; future BM25 uses **character bigrams**, with no tokenization dependency; Japanese additionally gets NFKC and kana normalization.
- Token estimation (rough, calibrate after landing):

```text
tok_est ≈ ascii_bytes / 4 + cjk_chars
```

- Mixed text is counted by characters, not guessed by bytes.

---

## 7. Monorepo and scope

- `search`/`glob` support `--scope apps/web`; when a task declares `paths`, only those paths are searched by default.
- Default ignores: `.gitignore` + `.loomignore` + `node_modules`, `target`, `dist`, `build`, `.venv`, `vendor`.
- Programming languages get tree-sitter connected gradually according to what actually appears in the repository; M1 does not pre-install the full grammar set.
- Multi-package config comes later; v1 only recognizes `.loom/config.toml` at the repository root.

---

## 8. Telemetry and tuning

Every tool call appends one line to `.loom/runs/<run>/logs/tools.jsonl`:

| Field | Description |
|---|---|
| `ts` | timestamp |
| `run` / `task` | ownership |
| `tool` | tool name |
| `args_hash` | argument hash; debug mode may record redacted args |
| `scope` | search scope |
| `items` | number of returned entries |
| `bytes` | envelope byte count |
| `tok_est` | token estimate |
| `truncated` / `spill` / `dedupe` | truncation / spill / dedup |
| `stale` | number of hash mismatches |
| `ms` | tool duration (goes only into the log, not the envelope) |
| `exit` | exit code |
| `budget_at` | the budget tier that was triggered |

Aggregated into the ledger:

- `tool_output_ratio` = tool output tokens / total input tokens;
- `spill_rate`, `truncation_rate`, `dedupe_rate`;
- `budget_hit` count;
- `tok_est_error`: calibrate the estimation coefficient against the provider's actual usage.

Tuning order: look at `tool_output_ratio` first, then `spill_rate`. If most calls spill, the default caps are too small or the agent searches too diffusely; if almost nothing spills but tokens are still high, the preview/entry caps can come down further.

**Privacy**: by default only paths, hashes, and sizes are recorded, never file content; `LOOM_TELEMETRY=debug` records redacted args, and they do not enter the shared knowledge base.

### 8.1 Headroom / cost-attribution measurement conventions

| Phenomenon | Correct reading |
|---|---|
| `WasteSignals` values are large | **cumulative detected volume**, not "already saved"; A/B only looks at the growth rate of `Δvalue / Δrequests` |
| `reread_compressed: 0` | the attribution logic is not in place yet, and this cannot be used as evidence that "compression is ineffective" |
| `share_of_saved_percent` >100% | the denominator is probably wrong; **disabled**, not used as an optimization target |
| `agent_usage` shows `codex` / `pi-extension` | may come from aggregate fallback; precise attribution is in `agents[].source` |
| `requests.by_client` is empty | cannot be the sole source of attribution |
| `mode` | **process-level config**, cannot be switched per request or per model; running both modes requires separate instances by client/port |
| client classification | look at `User-Agent` / `X-Client`, **not at model** |
| subscription-state compression | more conservative when `AuthMode=Subscription` (lossless-only / live-zone-only / lower `max_lossy_ratio`), needs separate accounting |

Rules:

- Headroom keeps running `mode=cache` and does not switch modes per model;
- `provider_cache_discount_usd` is recorded separately and never added to compression savings;
- subscription requests cannot use LiteLLM's `$0` spend for budgeting; use RPM/TPM or Loom's own `allowance_burn` instead;
- attribution headers (`X-Loom-Run` / `X-Loom-Task`) are used for the ledger only after Headroom confirms pass-through; when pass-through fails, attribute by process/task boundaries;
- subscription-related measurements (`allowance_burn`, subscription-state compression) wait for the subscription lane to be enabled; the current API-key multi-provider phase does not involve them.

---

## 9. Transport and Headroom integration

M0 does **CLI + internal API** only, no MCP:

```bash
loom tool search --q "refresh token" --scope apps/web --json
loom tool read --path apps/web/src/auth/session.ts --range 120:168 --json
loom artifact --ref artifacts/search-003.txt --range 1:120
```

Three ways to connect with Headroom, in priority order:

| Method | How | Assessment |
|---|---|---|
| **proxy** | the model request's `base_url` points at Headroom; Headroom then forwards to LiteLLM/provider | **first choice**: zero coupling, Loom is unaware |
| **wrap** | wrap Loom's HTTP client and add the `X-Loom-Run`/`X-Loom-Task` attribution headers | needs Headroom pass-through; used for cost attribution |
| **SDK** | call Headroom's library directly | last resort; avoid binding to the internal API |

The tool side has already brought the bytes down; Headroom keeps running `mode=cache` and only handles duplicate prefixes at the model layer. The two do not overlap.

---

## 10. Versioning

- `v:1`; incremental fields only, unknown fields ignored.
- Breaking change → `v:2`, never a silent semantic change.
- Schema file: `schemas/tool-result.v1.json` (JSON Schema 2020-12).
- Contract tests: every tool's output must pass schema validation; one envelope check is added to the adapter canary.
- `summary` template versions are recorded separately, so localization changes can be compared.

---

## 11. Examples

### 11.1 Truncated search results

```json
{
  "v": 1,
  "ok": true,
  "tool": "search",
  "summary": "12 matches in 4 files; returned the first 8",
  "items": [
    {"t": "match", "p": "apps/web/src/auth/session.ts", "b": "9f3c1a2b", "r": [120, 128], "s": "Session.refresh", "k": "def", "x": "export async function refresh(...)", "sc": 0.87}
  ],
  "ref": "artifacts/search-003.txt",
  "truncated": true,
  "next": "cur_7f3a",
  "stats": {"n": 12, "bytes": 1832, "tok": 420, "spill": true}
}
```

### 11.2 Repeat read

```json
{"v":1,"ok":true,"tool":"read","unchanged":true,"ref":"artifacts/read-002.txt","summary":"content unchanged; already returned in this run"}
```

### 11.3 File too large

```json
{"v":1,"ok":false,"tool":"read","summary":"file too large; specify a line range","err":{"code":"too_large","hint":"read --range 1:200"}}
```

### 11.4 Successful edit

```json
{"v":1,"ok":true,"tool":"edit","summary":"modified 1 file","items":[{"t":"file","p":"src/auth/session.ts","b":"3a7d9e01","a":12,"d":3,"n":2,"l":"ts"}],"stats":{"n":1,"bytes":186,"tok":48}}
```

### 11.5 Budget warning

```json
{"v":1,"ok":true,"tool":"search","summary":"3 matches","items":[{"t":"match","p":"src/lib.rs","b":"7c1e2a9","r":[42,42],"x":"pub fn refresh()","sc":0.9}],"stats":{"n":3,"bytes":140,"tok":36},"budget":{"u":98304,"l":131072,"p":75,"at":"run"}}
```

---

## 12. To do and to verify

1. **Calibrate token estimation**: compare `tok_est` against actual provider usage and fit separate English/Chinese/Japanese coefficients.
2. **Confirm key names in `.loom/config.toml`**: defaults go under `[tools.limits]`, hard caps stay in code.
3. **Confirm artifact naming and GC policy**: whether `<tool>-<seq>.txt` is enough, and whether content addressing is needed.
4. **Confirm the return granularity of `edit`/`write`**: stat only, or hunk headers (without full text).
5. **Where contract tests plug in**: schema validation in the tool wrapper or in the adapter normalization layer.
6. **Whether the native executor moves up into M1**: depends on the share of goose built-in tool output in the telemetry.
