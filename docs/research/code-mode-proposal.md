# Code Mode: proposal

Written 2026-10-19 for the "Design first" item in "After the opencode v2 review"
([engine-rewrite.md](../engine-rewrite.md)). What was known before is in "Code Mode in practice" in
[the v2 report](opencode-v2-comparison.md). No code until Kyle picks.

Status, 2026-10-20: the provider's own path shipped for Claude in 2.4 (deferred MCP tools behind an engine
`tool_search` returning `tool_reference` blocks, as Claude Code does; see "Prompt cache" in the engine
document). The other wires and the `execute` path are still open.

## The question

Should MCP tools stop being sent as native tool definitions, and if so, how: not at all, behind the
provider's own tool search, behind a Drift search tool, or behind an `execute` tool that runs
model-written JavaScript calling them?

## Measured

**Tool definitions per request.** Each definition serialised as `{name, description, input_schema}`
and counted in bytes; tokens are estimated at 4 bytes each. Built-ins from `Registry::builtin()`
(`crates/drift-engine/src/tool/registry.rs`) at this commit; MCP servers by running `tools/list`
against the same packages Kyle's `drift.db` configures (`chrome-devtools-mcp@latest`, `firecrawl-mcp`,
`https://mcp.context7.com/mcp`). The scripts are in the scratch directory, not committed.

| Set | Tools | Bytes | About tokens |
| --- | ---: | ---: | ---: |
| Built-ins, `edit` profile | 16 | 18,628 | 4,700 |
| Built-ins, `apply_patch` profile | 15 | 17,687 | 4,400 |
| chrome-devtools | 32 | 27,441 | 6,900 |
| firecrawl | 28 | 55,255 | 13,800 |
| context7 | 2 | 4,942 | 1,200 |
| Drift workspace (built-ins + the three) | 78 | 106,266 | 26,600 |
| Most other workspaces (built-ins + firecrawl + context7) | 46 | 78,825 | 19,700 |

The largest single definitions are `firecrawl_search` (8,729 B), `firecrawl_scrape` (6,826),
`firecrawl_agent` (6,105) and the built-in `bash` (3,044). MCP is 82% of the definition bytes in the
Drift workspace and 76% elsewhere. ida-pro-mcp (66 tools) and HvDebugger (36) are enabled in the RE
workspaces but need their targets running, so they were not measured. For scale, the median first
request of a Claude conversation in the Drift workspace since 2026-09-09 was 53,600 input tokens.

**MCP chains.** Every tool call in `drift.db`, read-only, 2026-03-26 to 2026-10-19. A turn is a user
prompt and the steps after it; fork copies are dropped by call id (14,133 turns before, 11,558 after).

- 365,088 tool calls, 17,660 of them MCP (4.8%): ida-pro-mcp 8,749, HvDebugger 3,998, firecrawl
  2,779, context7 1,393, chrome-devtools 642.
- 1,650 turns (14%) made an MCP call; 1,033 made three or more.
- 813 turns had a run of three or more MCP calls with no other tool between: 1,808 runs, median
  length 5. The results of all but the last call of each run total 36.1 MB (median 7.5 KB a run).
  This is the most Code Mode could keep out of context, and an overcount, because the model reads
  most of those results to decide the next call.
- A call that passes an id-like value (a digit, `/`, `:` or `_`, not in the prompt and not echoed
  from its own input) from the previous MCP result into its own input happened 713 times in 351
  turns: `resolve-library-id` to `query-docs` 72, `firecrawl_search` to `firecrawl_scrape` 60,
  scrape to scrape 29, HvDebugger `start_task` to `task_status` 26, `take_snapshot` to `click` 17.
  Those source results total 3.6 MB, median 1.7 KB.
- Runs where every step only carries a value forward: 3 runs in 3 turns, all ida-pro-mcp.

So the chains Code Mode exists for are rare in Kyle's work, and where values pass along the model is
usually still choosing among them (which search hit to scrape, which uid to click). The definitions
are the steady cost: on every request of every workspace.

**What the definitions cost when the cache is warm.** About 20 requests follow each user prompt
(147,404 replies to 7,433 prompts since 2026-09-09). At 21,900 MCP tokens (Drift workspace):

- Opus 5.5 cache read $0.20/M: $0.0044 a request, about $0.09 a prompt; a cold write ($5/M) $0.11.
- GPT-6.1 Sol cached input $0.10/M: half that.
- Across the 147,404 requests since September at 15,000 to 22,000 MCP tokens each: roughly 2.2 to 3.2
  billion cached tokens, $440 to $650 at Opus 5.5's cache-read price (API-equivalent; most of it ran
  on subscriptions).
- Context room: 22,000 tokens of a 400,000 or 1,000,000 window, 2 to 5%. Anthropic's tool search
  page says selection accuracy drops past 30 to 50 tools; the Drift workspace offers 78.

## The four paths

**1. Nothing.** Costs the numbers above and nothing else.

**2. The provider's tool search.** Anthropic: `tool_search_tool_regex_20251119` or `_bm25_`, every
definition still sent with `defer_loading: true`, the API searches and expands `tool_reference`
blocks, and "the prefix is untouched, so prompt caching is preserved" (`tool-search-tool` docs);
Claude 4.5 models and later; on Bedrock only through InvokeModel, which `llm/bedrock.rs` uses.
Responses has the same idea: a `tool_search` tool with `execution: "server"` or `"client"`,
`defer_loading` on functions, and `namespace` groups. Nothing runs model-written code. Not on Chat
Completions, Gemini or Ollama.

**3. Deferred MCP schemas behind a Drift search tool.** A built-in `tool_search` (query in, matching
MCP tools out). Where the wire can expand references, Drift uses it: Anthropic accepts
`tool_reference` blocks in a client tool's result (the documented custom search), Responses accepts a
client `tool_search_output`; the definitions are sent deferred and the prefix stays fixed. On other
wires the result carries the schema as text and the model calls a generic `mcp_call {server, tool,
input}`, so the tool list never changes mid-session; provider-side strict decoding is lost there.
Server instructions and a one-line index per server ("firecrawl: web search, scraping, crawling")
sit in the instruction baseline.

**4. `execute` with a JavaScript subset.** Typed signatures in the instruction baseline, one
`execute` tool, programs call `tools.<server>.<tool>()`. Candidate engines, each built as a release
`eval` binary on Windows here (stripped; an empty Rust binary is 0.13 MB):

| Engine | Version | Binary | Notes |
| --- | --- | ---: | --- |
| `rquickjs` (QuickJS-NG, C) | 0.14.0, MIT | 1.22 MB | interrupt handler for a hard deadline, memory limit, async host functions; builds with the MSVC toolchain the Rust build already uses |
| `boa_engine` (pure Rust) | 0.22.0, MIT or Unlicense | 12.9 MB, 14.3 MB with default features | `RuntimeLimits` on loops, recursion and stack; no interrupt hook for a wall-clock deadline |
| QuickJS compiled to Wasm in Drift's own wasmtime | not measured | not measured | epoch deadline and memory limits already used for plugins (`hook/wasm.rs`); only with the `wasm-plugins` feature |

Boa would add a quarter to Drift's 56 MB installer. opencode wrote its own interpreter (about 10,800
lines in `packages/codemode`).

| Point | 2. Provider search | 3. Drift search | 4. `execute` |
| --- | --- | --- | --- |
| Wires | Anthropic, Responses | All; native expansion on Anthropic and Responses | All |
| Permissions | Per call, as today | Per call, as today | Each nested call must go through the same ask with its real tool name and resource (v2 showed `*`, #55534); `execute` itself must be deniable (#56349) |
| Hard time limit | The tool's own, as today | The tool's own, as today | Required: QuickJS interrupt or wasm epoch, plus a deadline on pending host calls; v2 hung a worker for good without one (#56741) |
| Stop | As today | As today | Stop cancels pending host calls and interrupts the program; partial nested results are recorded |
| UI | Unchanged | A search row | `execute` row with nested call rows under it, each with its own ask and output |
| Servers with their own code mode (PostHog, executor.sh) | Defer like any tool | Defer like any tool | Must stay direct: a program calling a program (v2's `mcp-codemode-defaults.ts`) |
| Saving on a warm cache | Most of the $0.09 a prompt; context room; selection accuracy | Same, on every wire | Same, plus intermediate results kept out of context, which the chain numbers show is rare here |
| New cost | One search step on the 14% of turns that use MCP | Same; a wire adapter per expansion kind | 1.1 MB or more, a sandbox, limits, nested UI, and failures like `require('fs')` (#56811) |
| Rough size | 300 lines, 2 days | 700 to 900 lines, 4 to 5 days | 2,500 lines or more, 2 to 3 weeks |

## Recommendation

Path 3, deferring MCP definitions behind Drift's own `tool_search`, using provider expansion where
the wire offers it; no `execute` now.

It takes the definition cost that the measurements show on every request, on every wire, without
running model-written code. Path 2 alone covers only Claude and Responses; path 3 contains it.
`execute` pays for chains that happened three times in seven months. Revisit it if browser or
scraping work grows; the chain count above can be rerun read-only on the database to check.

## What would be built

- A per-server setting "Load tools on demand", on by default for a server over 10 KB of definitions
  or 10 tools, off for built-ins; drift.json `mcp.<name>.deferred` overrides it.
- `tool_search` (query, optional server, limit 5), permission kind `tool_search`, unasked.
- Anthropic: deferred definitions with `defer_loading: true` and `tool_reference` results, on API-key,
  subscription (check the `mcp_` name prefix in `claude_code.rs`), Bedrock and Vertex routes.
  Responses: client `tool_search` with `tool_search_output`. Elsewhere: `mcp_call` and text schemas.
- The baseline lists each deferred server's index line; a server connecting or a tool list changing
  becomes an instruction update, so no definition changes mid-session.
- Cache-prefix diagnostics (already planned) confirm the prefix stays fixed after a search.
- Tests: a search returns references on Anthropic and Responses and text on Chat Completions; a
  found tool runs with its own ask; the request prefix is byte-identical before and after a search.
