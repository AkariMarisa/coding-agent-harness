# 09 · Code Search and Knowledge Base (Assessment Notes)

> Language: **English** | [简体中文](09-code-search-and-knowledge-base.zh-CN.md)

> Status: **discussion draft / assessment notes**, not an implementation plan.  
> Purpose: answer one question — whether local code RAG / a knowledge base is worth bringing into Loom, and how to bring it in so it does not become a net loss.  
> Conclusions will be written back into the pending items of [08-design-roadmap-v0.2.md](08-design-roadmap-v0.2.md); M0 does not grow its implementation scope because of this document.

> **2026-11-04 decision update**
>
> 1. **v1 introduces no embedding model and no vector store.** Client performance and distribution size come first; retrieval starts with deterministic options.
> 2. **Semantic retrieval is deferred**, and is re-evaluated only when all of "model ≤ about 100MB, pure CPU, query < 100ms, optional download, clear A/B benefit" hold at the same time; vLLM is a server-side option and does not ship with the CLI.
> 3. **The knowledge base is this project only by default**; cross-project sharing must be explicitly enabled by the user.
> 4. **Tool-side I/O budgeting takes priority over gateway compression**: tokens that can be cut at the tool boundary are not left to Headroom.

---

## 1. Conclusions first

1. **A code vector store is not recommended as the main retrieval path.** The bulk of code queries are exact symbols, call chains, types, and paths, not "semantically similar segments"; vector retrieval is both expensive and inaccurate on such queries.
2. **Split the problem into two things**, whose update strategies differ completely:
   - **Code Index**: a derived cache of the current working tree; it must be fresh, and returns only **addresses** (`path` + line number + symbol); deterministic, deletable and rebuildable.
   - **Knowledge Base**: project memory (receipts, decisions, failed attempts, ADRs, issues), append-only, bound to commits, **does not need to be real-time**.
3. **The goal of code retrieval is not "find similar code" but "let the agent locate the right definition/call site with the fewest tokens."** So the first priority is structured search (trigram, symbol graph, repo map), not embedding.
4. **Freshness does not come from "real-time rebuilding" but from "content addressing + incremental invalidation + read-time verification."** The index may lag briefly, but any result returned to the model must be able to prove that it matches the current file content.
5. **The real high-ROI area for semantic retrieval is "what is not in the repository"**: why a change was made this way, how it failed last time, where an acceptance criterion came from. These are exactly the receipts / decisions / events that Loom is already generating.
6. **The first thing to build is not retrieval, but tool-side budgeting.** Changing tool output from "values" to "references + summaries" directly attacks `json_bloat` and `reread`, and depends on no index or model.

In one sentence: **code uses the index, history uses the knowledge base; the index hands out addresses, the compiler decides how much to read; slim down at the tool first, then let the gateway fall back.**

---

## 2. Why "code RAG" tends to look useful in development while saving nothing in practice

| Failure mode | Mechanism | Consequence |
|---|---|---|
| **Chunking breaks structure** | functions/types get cut, losing imports, generics, call relationships | the model sees a fragment and still does not know whether it can be changed or who is affected |
| **Precision mismatch** | code queries are mostly identifier / type / call chain; vectors excel at fuzzy similarity | "find all callers of X" is almost certainly wrong, and you end up back at grep |
| **Freshness trap** | the index lags the working tree and hits deleted/renamed code | worse than no index: the model gains confidence in the wrong context |
| **Self-deceiving evaluation** | evaluating with "recall/similarity" instead of "did the task pass verify faster" | the metrics look good, TPVT/WPT do not drop |
| **Cost shifting** | building the index, computing embeddings, chunk stuffing all cost money | the context grows every turn, offsetting the gain from "reading fewer files" |
| **Distribution and privacy** | local embedding models, vector stores, version drift | the single binary gets heavy, offline/intranet scenarios get complicated |

A common tendency in recent evaluations is: **on real repositories, agentic search (grep/read + iterative narrowing) usually beats pure vector retrieval**; the advantage of vector retrieval lies more in docs, issues, and natural-language knowledge than in precise code location. But this conclusion depends on task type and repository size, **it cannot be copied directly, and must be reproduced with Loom's own task set**.

---

## 3. But agentic grep is not free either

Rejecting "code RAG as the main path" does not mean today's grep approach is good enough. Its costs are:

- **Every search round is an LLM call**, and the history is resent; the locating phase may take a large share of total tokens (needs measurement).
- **Noise is heavy on large repos**: `rg "config"` may return thousands of hits, and the agent spends many rounds narrowing down.
- **No global view**: without a symbol graph / repo map, the agent easily fixes only the symptom and misses the real definition site or all the callers.
- **Tool results are pasted back by value**: raw `rg --json` or whole-file output directly inflates `json_bloat` (the biggest waste source in the earlier measurements, 60.6M).

So the opportunity is not "replace grep with RAG", but: **give search a better index and ranking, and compress the results into addresses.**

---

## 4. The key split: Code Index vs Knowledge Base

| Dimension | Code Index | Knowledge Base |
|---|---|---|
| Data | source code of the current working tree / commit | receipts, decisions, failures, ADRs, issues, commit messages |
| Change frequency | every edit | append-only |
| Freshness requirement | **strong** (verified at query time) | **weak** (versioned by commit/date) |
| Indexing method | trigram, tree-sitter symbol table, import/call graph | BM25/FTS5 + metadata filtering (v1 uses no embedding) |
| Invalidation strategy | git blob hash + incremental rebuild | never delete, append a new version |
| Typical failure | stale hit | recall noise, outdated conclusions |
| Main value | lower locate tokens / fewer useless reads | avoid re-hitting the same pitfall, re-implementing, re-discussing |
| Conclusion | **determinism first, no reliance on embedding** | **the main battlefield for semantic retrieval, but v1 uses BM25 first** |

> Source code is already in the repository and reachable by grep; what is genuinely "not in the repository" is the **why** and **what happened last time**.  
> So if only one thing gets built, build the knowledge base first; if code retrieval gets built, build the symbol index first.

---

## 5. Ready-made solutions worth borrowing from

| Solution | Mechanism | Use in Loom | Note |
|---|---|---|---|
| **ripgrep / git grep** | regex + path filtering | R0 default, kept forever | output must be structured and capped |
| **zoekt** | trigram inverted index, code-specific | fast candidate recall on large repos | needs an extra process/index directory |
| **tree-sitter tags** | AST extraction of definitions/references | R1 symbol table, incremental parsing | multi-language grammar maintenance |
| **Aider repo map** | symbol graph + PageRank + budgeted selection | R1 global view / ranking | borrow the algorithm only, no Aider dependency |
| **LSP / SCIP / LSIF** | precise jump, call hierarchy | optional at M4, precise but heavy | high startup/memory cost |
| **BM25 (FTS5/Tantivy)** | keyword relevance | R2 doc/history retrieval | also works on code identifiers |
| **local embedding** | semantic similarity | **not used in v1**; only an optional plugin later | size, performance, version, privacy |

**Ranking suggestion**: `exact symbol > trigram > BM25 > vector`. Vectors only supplement hybrid recall and never decide the result on their own.

---

## 6. Layered retrieval: R0–R3

| Layer | Content | Freshness | When to enable | Maintenance cost |
|---|---|---|---|---|
| **R0** | `git ls-files`, `rg`, `git grep`, path/extension filters | real-time | always | near zero |
| **R1** | tree-sitter symbol table, import graph, repo map, PageRank ranking | verified at query time + incremental | M1, if locate cost is high | medium |
| **R2** | BM25/FTS5 + metadata filtering (path/language/commit); code only as fallback | versioned by commit | M2, docs/history first | medium |
| **R3** | issue/wiki/MCP external knowledge | loose | M3+ | depends on the source |

Rules:

1. **R0/R1 return addresses only**, never large chunks of content.
2. **After R2 returns candidates they must be resolved to addresses**, and verified against the content hash before entering the context.
3. Any layer failing **degrades to R0** and must not block dispatch.
4. The model never sees raw `rg --json` directly; everything goes through `loom search`'s compact protocol.

**R2 in v1 contains no embedding**: BM25 + exact identifier matching + tag/time filtering already covers queries like "which task touched here last time" and "why was this decision made", with zero models and zero GPUs.

---

## 7. "Addresses, not content": the retrieval result protocol

```json
{
  "path": "src/auth/session.rs",
  "blob": "9f3c1a...",
  "range": [120, 168],
  "symbol": "Session::refresh",
  "kind": "definition",
  "score": 0.87,
  "why": "caller of rotate_token; touched by T-003",
  "est_tokens": 740
}
```

Conventions:

- **`blob` is a content hash** (`git hash-object` or equivalent), not an mtime.
- **`est_tokens` is estimated by the index**, and the Context Compiler uses it to decide which ones to expand and how much.
- **`why` is auditable**: convenient both for human troubleshooting and for evaluating precision.
- **seen-set dedup**: a `(blob, range)` already sent within the same run is not resent; only a diff or a one-line summary goes out.
- **Read-time verification**: re-read the file and compare hashes before expanding; on mismatch, discard, re-query, and record a `stale_hit` event.
- **No expansion by default**: hand out the address list first (a few dozen tokens), and the model reads by range when it needs to.

This protocol solves three old problems at once: `json_bloat`, `reread`, and cross-task repeated reads.

---

## 8. Tool-side budgeting: a layer earlier than compression

> This section is the highest-priority part added by this round of discussion.  
> In the earlier measurements, Headroom compression saved only **0.70%**, while `json_bloat` was **60.6M** and `reread` was **5.67M**.  
> Conclusion: **control tool I/O first, then talk about compression and retrieval.**  
> Fields, limits, and telemetry are in [10-tool-output-protocol-v0.1.md](10-tool-output-protocol-v0.1.md).

### 8.1 Principles

1. **Whatever can be cut at the tool boundary is not left to the gateway or the model.**
2. **Tool output is a reference, not a value.** By default return "summary + address + artifact path", and spill the full text to disk.
3. **Hard budget caps.** Every tool, every call, and every run has a byte/token cap, and the model cannot bypass it through arguments.
4. **Deterministic truncation.** The same input produces the same truncated result, which makes caching and reproduction easy.
5. **The gateway is the fallback.** Headroom keeps running `mode=cache` and only handles duplicate prefixes at the model layer; the tool side brings the bytes down first.

### 8.2 Output protocol (sketch)

```json
{
  "ok": true,
  "summary": "12 matches in 4 files",
  "ref": ".loom/runs/R-001/artifacts/search-001.txt",
  "items": [
    {"path": "src/auth/session.rs", "range": [120, 128], "preview": "fn refresh(...)"}
  ],
  "truncated": true,
  "next": "loom search --cursor eyJvZmZzZXQiOjUwfQ",
  "bytes": 1832
}
```

Rules:

- **Default caps (examples, calibrate when landing)**:
  - `search`: 50 hits / 8KB;
  - `read`: 200 lines / 8KB, beyond that must continue reading by range;
  - `tree`: depth 3 / 200 entries;
  - `git diff`: 500 lines, beyond that return stat + artifact;
  - `test/verify`: last 200 lines + a reference to the full log.
- **Over the cap means spill to artifact**: artifacts go in `.loom/runs/<run>/artifacts/`, and the result returns only `ref` + the top few entries.
- **Dedup**: the same `(path, blob, range)` already sent this run → return `{"unchanged": true, "ref": ...}` instead of resending content.
- **No raw JSON/HTML/ANSI/progress bars**; the adapter is responsible for normalization.
- **Errors return only**: exit code + one-line reason + optional `ref`; no stack traces.
- **Field order is stable**, which helps prefix caching.

### 8.3 Input protocol

- Tool schemas carry their own defaults and hard caps: `max_results`, `max_bytes`, `context_lines`, `path_glob`.
- **Expensive calls are rejected**: a repo-wide grep with no path, a whole-file read, a recursive directory listing with no depth limit → automatically downgraded to a restricted call, with an explanation in the result.
- The model's tool call arguments are normalized before execution; the budget cannot be left to prompt constraints and must be enforced by the wrapper.
- **Total tool output budget per run** (example 64KB / 16k tokens); once over budget only `read --range`, `verify` log tailing, and necessary writes are allowed.

### 8.4 Relationship with Headroom

| Layer | Responsible for | Not responsible for |
|---|---|---|
| tool wrapper | value → reference, truncation, dedup, structure | model-layer duplication, cross-turn caching |
| Context Compiler | which addresses go into the prompt, budget allocation | large results inside tools |
| Headroom | caching, compressible duplicate content | tool output governance |

They do not overlap and do not replace each other. What the tool side saves is **real bytes**; what the gateway saves is **duplicate sends**.

### 8.5 Why this comes first

- no index, no model, no new dependency needed;
- directly attacks `json_bloat` and `reread`;
- works for any adapter (goose / OpenCode / native all go through the same wrapper);
- it is a prerequisite for the Context Compiler: make tool results small first, then talk about compilation.

---

## 9. Freshness: not real-time, but provably consistent

The core worry is "the source code keeps changing, does the index have to update in real time". The answer: **no real-time full rebuild is needed; provable consistency is needed.**

1. **The index is not the source of truth.** The source of truth is the git object + working tree; the index is always disposable and rebuildable.
2. **Content addressing.** The cache key uses `path + blob_sha`, not mtime; renames are recognized with `git diff -M`.
3. **Read-time verification.** After a hit, read the actual file and compare hashes; on mismatch, invalidate. That way, even if the index lags by a few seconds, stale content is never fed to the model.
4. **Incremental updates.**
   - before dispatch: `git status/diff` yields the dirty set;
   - after the worker finishes: `git diff --name-status` rebuilds only the changed files;
   - large repos / daemon: background debounce, triggered on demand, never blocking dispatch.
5. **Versioning.** The index schema and parsers both carry versions; on mismatch, rebuild, never mix versions.
6. **Boundaries.** Respect `.gitignore` + `.loomignore`; skip vendor/generated/build/binaries; cap single-file and total sizes.
7. **Multiple worktrees.** Prefer blob content addressing, which is shared across worktrees by nature; only the path mapping differs per worktree.
8. **The consistency window is acceptable.** The index may lag briefly, but **every address returned to the model must pass read-time verification**.

> Conclusion: replace "real-time index" with "content addressing + incremental + read-time verification", and the complexity drops from "continuous synchronization" to "invalidation on demand".

---

## 10. Interfaces with Loom's existing design

| Where | How it plugs in |
|---|---|
| tool wrapper | tool calls of all adapters go through the same layer: caps, structure, spill to artifact, dedup |
| `context.compile()` | calls retrieval with the TaskSpec and gets an address list; the Compiler decides expansion and budget |
| worker tool surface | provides `loom search` (structured, capped, returns addresses) instead of raw grep; optionally `loom read --range` |
| adapter normalization | unifies the agent's search results into the hit structure above, filtering raw JSON |
| receipt | records `retrieval` and `tool_io` statistics (queries, hits, stale, bytes, truncated), which enter the ledger |
| `.loom/index/` | gitignored, deletable, rebuildable; not in snapshots, not in worktrees |
| `loom doctor` | checks index version/hash/orphan entries; failures only warn, never block |

Key point: **retrieval is a submodule of the Context Compiler, not a standalone product.** It is only responsible for "candidates + ranking + addresses", not for "stitching content into the prompt".

---

## 11. Roadmap: what to do when

| Stage | Does | Does not | Exit criteria |
|---|---|---|---|
| **M0** | R0 (`rg` + range reads) + **tool output budgeting/spill/dedup**; record retrieval traces | no index, no embedding/vector store | locate cost and tool bloat baselines in hand |
| **M1** | if traces show locating is a large share: add the R1 symbol table + repo map; `loom search` compact output | no semantic retrieval | locate tokens / tool calls drop noticeably, verify does not regress |
| **M2** | if failures concentrate on "cannot find the relevant implementation / duplicate implementation": add R2 (**BM25/FTS5 only** + metadata, knowledge base first) | semantic similarity never decides the main result | A/B shows TPVT or first-pass improving |
| **M3** | cross-project knowledge base (explicitly enabled by the user), issues/ADR, historical receipt retrieval | no indexing of the entire git history | reuse rate / repeated pitfalls drop |
| **M4** | optional LSP/SCIP/zoekt-grade precise index | not built in-house in the first phase | clear benefit on large-repo scenarios |

**Explicitly not doing in M0**: embedding, vector stores, LLM-generated index summaries, a file-watching daemon.

---

## 12. How to prove it works (A/B)

| Metric | Definition | Note |
|---|---|---|
| `locate_cost` | tool calls + tokens from task start to first read of the "right file" | most direct |
| `tool_output_ratio` | tool output tokens / total input tokens | look at this first |
| `spill_rate` | fraction of calls that spill to artifact | too high means the default caps are wrong |
| `retrieval_precision@k` | fraction of returned addresses that were actually read and ultimately used for the modification | not recall |
| `wasted_reads` | number/bytes of files read but not used for the modification | the counter-metric |
| `stale_hits` | number of hash mismatches | freshness health |
| `TPVT` / `WPT` | billed tokens, wall clock / tasks passed | the final metrics |

Experiment design:

- 20–30 real tasks, same model, same prompt, same verify;
- run the "tool budget on/off" A/B first, then R0 vs R0+R1;
- **verify pass** is the hard metric, tokens/wall clock are the cost metrics;
- record a trace for every locate, so averages alone do not decide.

**Anti-patterns**: proving effectiveness with "retrieval hit rate" alone; having an LLM generate summaries/embeddings and then storing them (expensive, drifting, unreproducible).

---

## 13. Anti-pattern list

1. Cutting whole files into fixed chunks, storing them as vectors, and stuffing 10 chunks into the context after a query.
2. Having an LLM generate a summary for every file and then indexing it (expensive, drifting, unreproducible).
3. Making the index a mandatory path; if the index breaks, tasks cannot run.
4. Treating index results as fact without verifying hashes.
5. Shipping a vector store / embedding service in M0.
6. Replacing exact symbol queries with semantic similarity.
7. Indexing the entire git history/blame (size explosion); index only receipt/decision summaries.
8. Letting the worker see raw `rg --json` directly.
9. Leaving tool caps to prompt constraints instead of enforcing them in the wrapper.

---

## 14. Decided and open

### 14.1 Decided

| Item | Decision | Reason |
|---|---|---|
| embedding / vector store | **not in v1** | client performance, distribution size, offline and privacy |
| semantic retrieval | **deferred, optional plugin** | must clear the size/CPU/latency/A-B thresholds at the same time |
| knowledge base sharing | **this project only by default**, users can explicitly enable cross-project | avoids default leakage and index bloat |
| Priority | **tool-side I/O budget > retrieval index > gateway compression** | cut real bytes first, then talk about compression ratio |
| R2 implementation | BM25/FTS5 + metadata filtering | zero model, explainable, reproducible |

### 14.2 Open / needs spike

1. **Scale and language of the target code repositories** (this means the projects Loom runs on, not the knowledge base): single repo/multi-repo, monorepo, multi-language; these decide R1 parser coverage and whether zoekt is worth it.
2. **Size cap for `.loom/index/`**: whether a few hundred MB is acceptable, or whether it goes into a temp directory only.
3. **`loom search` output protocol**: recommend designing our own; reusing `rg --json` would carry noise into the context.
4. **Threshold for future embedding**: model ≤ about 100MB, pure CPU, query < 100ms, optional download (with checksum), clear A/B benefit; runtime preference for llama.cpp / ONNX Runtime / candle, and vLLM does not ship with the CLI.
5. **Whether Windows is in scope**: affects file watching, long paths, process groups.

---

## 15. One-sentence summary

**What code retrieval must do is "locate more cheaply", not "similarize more cleverly"; the index must be a disposable, content-addressed, read-time-verified derived cache; and the first cut should land on tool I/O, not gateway compression.**
