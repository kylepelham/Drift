# Compaction by the provider: proposal

Written 2026-10-19 for the "Design first" item in "After the opencode v2 review"
([engine-rewrite.md](../engine-rewrite.md)). What was known before is in "Compaction by the provider"
in [the v2 report](opencode-v2-comparison.md). No code until Kyle picks.

## The question

Should Drift let OpenAI or Anthropic compact a long conversation instead of writing its own summary
(`session/compaction.rs`: a one-shot summary request, the last two turns kept verbatim within 2,000
to 15,000 tokens, the transcript kept whole and the request view skipping what was summarised)?

## What the providers offer now

**OpenAI** (`guides/compaction`, API reference):

- Standalone `POST /v1/responses/compact`: send the window, get back a new window that contains an
  encrypted `compaction` item plus retained items; pass it on unpruned. Stateless, ZDR-friendly.
- In-request `context_management: [{ "type": "compaction", "compact_threshold": N }]` (minimum 1,000)
  on `POST /responses` and the Responses WebSocket: the server compacts when the rendered input
  crosses N and streams the `compaction` item in the same response.
- A `compaction_trigger` input item asks for compaction on demand inside a normal request
  (`response.compaction.compacting` reports progress; codex#22809).
- The item "is opaque and not intended to be human-interpretable"; billed as ordinary tokens.

**Anthropic** (`build-with-claude/compaction`, `-threshold`, `-on-demand`):

- Threshold: the `compact_20260112` edit in `context_management` (beta `compact-2026-01-12`), default
  trigger 150,000 input tokens, returns a `compaction` block with readable `content` (null when it
  failed), `encrypted_content`, `signature` and `tool_changes`. Its tokens sit in `usage.iterations`
  as a `compaction` entry, not in the top-level fields. Claude API, Bedrock, Vertex, Foundry: beta.
- On demand, new since the report: top-level `"compaction": { "type": "summarize" }` (beta
  `compact-2026-09-04`) makes the request a compaction request; the reply is one signed block, which
  later requests send first in place of the messages it covers. Top-level usage is zero; sum
  `usage.iterations`. The block must come back unedited (`compaction_signature_invalid`). The
  summary uses the request's own model; images, documents and fetched URLs in the covered range are
  gone afterwards. Not listed for Bedrock.

## Answers, as far as the docs and public reports go

**ChatGPT sign-ins as well as API keys.** On the Codex backend (`chatgpt.com/backend-api/codex`)
`/responses/compact` now answers 404 for every model (codex#38323; codex#42468, 2026-09-02; Zed's
PR #62547). The same PR verified `context_management` and `compaction_trigger` returning a
streamed `compaction` item over ChatGPT OAuth for gpt-5.4 through gpt-5.6; Codex 0.153 uses that
path ("remote compaction v2"). So: in-request yes, standalone no. The standalone call is also unary,
and a large one has hit Cloudflare's 300 s idle cut (openclaw#86019). Anthropic: the docs cover API
keys and the clouds; whether the Claude Code subscription route accepts either beta header is not
documented and must be checked with one request.

**An encrypted item and a move to another account.** It fails. OpenAI encrypted content is bound to
the organization that made it (`invalid_encrypted_content`, "organization_id did not match"; codex
#36704 broke whole threads after a key rotation). Drift already sends a reasoning signature back only
to the model and account that made it (`session/convert.rs`, `OnCatalog`). Reasoning can be dropped;
a compaction item cannot, because it is the only carrier of the older history. Kyle does move: the
longest Claude conversation in the Drift workspace ran on three accounts. Anthropic's signed block
is presumably bound the same way (not documented), but its `content` is plain text, so it can be
sent as an ordinary summary when the signature cannot.

**Undo, fork and the context meter.**

- Undo: in-request compaction lives inside an assistant reply, so undoing that turn removes it and the
  older messages come back into view, as undoing a Drift compaction does now. No change needed.
- Fork: the copy carries the item; it works only while the fork keeps the same account. A fork moved
  to another account needs a fresh summary first.
- Meter: the local estimate (4 characters a token since the latest summary) cannot size an encrypted
  item. The meter must use only the provider's reported usage after a native compaction, as v2 does
  (`hasMeasuredPrompt`), and for Anthropic read the last `message` entry of `usage.iterations`.
  Cost tracking must sum `iterations`, or a threshold compaction is not charged.

**Anthropic's readable block standing in for Drift's summary.** Yes, on Claude. The on-demand form is
the closer fit: Drift already decides when to compact and already sends one separate request; the
block's `content` becomes the summary message's text (shown, counted and carried by Drift as now)
and the signed block rides as the part's `provider_signature`. On a model switch within the same
Claude model and account the block is replayed; to any other model or account the text is sent the
way Drift's summary is sent today. Nothing is lost on a switch, unlike OpenAI's item, which on a
switch needs a second compaction from the full transcript (v2's behaviour).

## Measured comparison: not yet measured

No paid requests were made for this proposal. The run, when Kyle approves the spend:

1. Sessions: `ses_f05eb665cffeo9OnCqKKTo0hin` ("Implement Drift here", Claude Opus 5.5, 4,755 messages, 9
   Drift summaries, median input 642,000 tokens, $22.88 of summaries at API prices) and
   `ses_05566b267ffemEpNAeKOZvEqJc` (GPT-6.1 Sol and others, 6,326 messages, 48 summaries, median input
   87,400 tokens, $3.62).
2. For each recorded compaction point, rebuild the exact request Drift sent (cache-prefix
   diagnostics give the bytes), then ask the provider: Anthropic on-demand `summarize`; OpenAI
   `compaction_trigger` with `store: false`. Drift's own summary at that point is already in the
   database, with its cost.
3. Continue from each of the three windows with the next real user prompt, tools stubbed to replay
   the recorded results, for up to five steps; then ask five fixed probes (files changed so far, the
   current step, open todo items, the last failing test, the user's latest constraint), graded
   against the transcript.
4. Record per point: compaction cost, wall time, next-step agreement with the recorded call, probe
   score.

Cost at list prices with cold caches: Claude, 9 points of about 642,000 input tokens at $4/M plus
about 6,000 output at $20/M, about $24 for compaction and about $15 for the continuations and probes;
GPT, 48 points at about 87,000 tokens on GPT-6.1 Sol ($2/M in, $10/M out), about $10 plus $10. Total
about $60. Run on Kyle's subscriptions instead, it costs plan usage, not money.

## Recommendation

Keep Drift's summary as the authority on every provider. Run the measurement, and if Anthropic's
on-demand block scores no worse, use it as the summary writer on Claude routes that accept the beta.
Do not adopt OpenAI's encrypted item by default: it cannot be shown, measured locally or moved
between accounts, and the Codex backend has already retired one of its two entry points.

## What would be built (Anthropic, if picked)

- A catalog capability `compaction_on_demand`, set from the Models API `capabilities.compaction`.
- In `session/compaction.rs`, the summary request becomes a `summarize` request on those models,
  with the beta header on it and on every later request that carries the block.
- The block is stored on the summary message: `content` as its text, the rest as
  `provider_signature`; `convert.rs` replays the block only for the same model and account.
- Usage reads `iterations`; a null `content` counts as a failed compaction (today's three-strikes
  rule applies).
- Tests with recorded responses: replay to the same model sends the block; a switch to GPT sends the
  text; a different account sends the text; a null block falls back to Drift's own summary.

Rough size: 400 to 600 lines with tests, about three days, plus the measurement run (a day, mostly
script). OpenAI in-request compaction, if later wanted: about 600 lines, plus the forced
re-summarisation on an account or model move.
