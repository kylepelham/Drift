# Claude 2.1.85 binary evidence

This report examines the installed `.local/bin/claude.exe` as bytes. It does not execute extracted code. All offsets are decimal file offsets, with end-exclusive ranges. The executable is 237,718,176 bytes and its SHA-256 is `4a336dc188dc44289c801a33ba1868fa2b2b39d345e9a3316327218048c91669`. Findings apply to this exact file, not other Claude releases.

## Reproduce

From the repository root, run:

```powershell
$binary = Join-Path $HOME '.local/bin/claude.exe'
$temp = Join-Path ([IO.Path]::GetTempPath()) 'opencode'
New-Item -ItemType Directory -Force -Path $temp | Out-Null
bun scripts/inspect-claude.ts --help
bun scripts/inspect-claude.ts --input $binary --output (Join-Path $temp 'claude-2.1.85-manifest-recheck.json')
bun scripts/inspect-claude.ts --input $binary --literal '// @bun @bytecode @bun-cjs' --literal '//# sourceMappingURL=' --literal 'B:/~BUN/root/' --literal 'Bun v1.' --literal 'JSC::' --output (Join-Path $temp 'claude-2.1.85-markers-recheck.json')
bun scripts/inspect-claude.ts --input $binary --probe 120866728
bun scripts/inspect-claude.ts --input $binary --probe 120867360
bun scripts/inspect-claude-source.ts --input $binary --range 120866728:133121000 --output (Join-Path $temp 'drift-claude-source-index-recheck.json')
bun test tests/inspect-claude.test.ts tests/inspect-claude-source.test.ts
bun run typecheck
```

The recorded runs wrote `claude-2.1.85-manifest-drift-v3.json` and `claude-2.1.85-markers-drift.json` under that temp directory. A review pass with exact reads, chunk-boundary identifier checks, and corrected overlay handling wrote `claude-2.1.85-inspector-reviewed.json`; it reproduced the hashes, section ranges, 167 readable runs, and 324 distinct identifier names across 2,154 occurrences. New report files must not already exist. Omit `--output` for stdout. A private byte-range export is opt-in, limited to 16 MiB and restricted to an explicit temp destination:

```powershell
bun scripts/inspect-claude.ts --input $binary --extract 120866728:133121000 --output (Join-Path $temp 'claude-2.1.85-first-source-recheck.bin')
```

Do not check that export into git. The tool reports its hash but never runs it. `--probe` displays 96 bytes, so choose offsets deliberately. The manifest keeps only byte offsets, 128-byte-ish window hashes, the 30 largest printable-run hashes, and the 80 most frequent identifier names. It contains no bulk source.

## Container and payload

The PE parser finds a 64-bit x86 PE32+ image (`machine = 34404`, `0x8664`) with 12 sections. The critical raw ranges are `.text` `[1024, 63678976)`, `.rdata` `[63678976, 113690624)`, and `.bun` `[120866304, 237707776)`. The full section table, including virtual addresses and raw extents, is in the manifest. The security directory points to `[237707776, 237718176)`, 10,400 bytes, immediately after `.bun`. The section-based overlay is exactly that same range. This identifies the certificate location in PE metadata; it does not validate the signature.

Within `.bun`, `[120866728, 133121000)` and `[223676816, 235931088)` each contain 12,254,272 bytes of uninterrupted printable ASCII and whitespace. Both entire ranges have SHA-256 `a96b01820b7d0036d1d0bf31563cf5d368f586f62f7d655339fbcdd9cadf43d9`. A 96-byte probe at either start returns the same hash `16c769bba96795efeef8367096b28f47a6a756b90a06ecbcf98dc824febade35` and begins `// @bun @bytecode @bun-cjs` followed by a CommonJS wrapper and Claude comment. The first copy contains `// Version: 2.1.85` at offset 120867392 (literal `2.1.85` at 120867404), followed by minified `var` declarations. The second copy starts 102,810,088 bytes after the first. This proves two byte-identical, large, readable bundled-source runs; it does not prove why Bun stores both copies or whether the runtime loads both.

The full-file ASCII/whitespace run scan, with a 4,096-byte minimum, found 167 runs totaling 28,300,805 bytes. Subtracting the second identical run leaves 16,046,533 bytes of counted printable runs, but that figure is **not** a completeness estimate for recoverable source. Runs below 4 KiB, UTF-8 multibyte text, compressed content, bytecode strings, and mixed binary/source records are omitted. The remaining `.bun` section has binary data and embedded path markers, including `B:/~BUN/root/image-processor.js` just after the second long run. Bytes at the end of the first run switch to non-printable binary. The `@bytecode` comment is a Bun wrapper marker, **not** proof that the 12 MiB text itself is serialized bytecode. The runtime also has JSC symbols and strings in `.rdata`; deriving bytecode format or executable semantics would need format-level work.

## Search results and fingerprint windows

The scanner traverses the whole file in 1 MiB owned chunks with a 4 KiB lookahead. Each occurrence is counted at its starting offset once. Matching is ASCII/Latin1 so character indices equal byte offsets. Windows hash up to 64 bytes on either side of a match; only the first eight matches per marker get windows. A few useful anchors:

| Literal | Full-file count | Example byte offset | SHA-256 of window |
| --- | ---: | ---: | --- |
| `2.1.85` | 326 | 120867404 | `85749890230eab4a8c1df178491512ddcc968f39b8911dc1297d3a4d2fae65bf` |
| `CLAUDE_CODE_` | 1309 | 120913174 | `0bd068033093f20b98d3ea45495179460eab6614bf6b2f63ba88cae7b7786056` |
| `ANTHROPIC_` | 578 | 120987724 | `3ab5eebb23dc97e8dcaa6d3efb2334024c317cec1f089b5841ecfda24f8adede` |
| `// @bun @bytecode @bun-cjs` | 5 | 120866728 | `72ffe977c45a5a83a2a20e06e0e091ccb663c9f18340bb37f9a16a18e7771f82` |
| `B:/~BUN/root/` | 15 | 120869322 | `bab4fb9b6dfeb942cda7cd24eae41821c4b73310102b16a4aa77aff872f2152d` |
| `//# sourceMappingURL=` | 5 | 65690154 | `1b2b4c457e4a7d431d1ff974aa71bbc9d4482b6c7579b474ba525be249bb44d1` |

The five `//# sourceMappingURL=` hits are at 65,690,154, 65,690,330, 68,333,171, 68,804,646 and 68,858,798, all within `.rdata`, rather than within the two long source runs. A marker may occur in a runtime string describing source maps. This search does not establish an embedded source map, nor does it rule out maps stored under another representation. `BUN_BE_BUN` and the exact ASCII `source-map` had zero hits; those are narrow negative searches, not format exclusions. `JSC::` appears 7,688 times in the additional marker scan, consistent with runtime symbols/strings, not a count of application modules.

The identifier scan matches only contiguous uppercase tokens with prefixes `CLAUDE_CODE_`, `ANTHROPIC_`, `BUN_`, `DISABLE_`, or `ENABLE_`. It found 324 distinct names across 2,154 occurrences. Group counts are 172/1,303, 32/578, 84/113, 22/107, and 14/53 respectively (distinct/occurrences, in that prefix order). Examples include `ANTHROPIC_API_KEY` (176 occurrences), `CLAUDE_CODE_ENTRYPOINT` (65), `CLAUDE_CODE_USE_BEDROCK` (40), and `ENABLE_TOOL_SEARCH` (28). These are names only; no actual environment values or credentials were read. A token match is not proof of a live flag, an environment read, or a particular branch condition. Prefix literal counts can exceed token counts because not every prefix occurrence completes the identifier pattern.

## Parsed source index

`scripts/inspect-claude-source.ts` reads an explicit range of at most 16 MiB and hashes the whole input file plus that range. It decodes the selected bytes as Latin1, preserving one character per byte for file offsets, then parses the text with the existing TypeScript dependency in JavaScript mode. It writes an index only to the supplied fresh `--output` file. The index has function names/kinds/ranges/parent offsets, direct syntactic identifier or property-call references, `tengu_` literal labels grouped by callee, and `process.env` references. It does not store source text, argument expressions, unrelated string literals, or diagnostic excerpts. Top-level calls have their own list. Named direct env references and bracket/dynamic/bare `process.env` uses are counted separately.

For the first source range `[120866728, 133121000)`, the source hash is `a96b01820b7d0036d1d0bf31563cf5d368f586f62f7d655339fbcdd9cadf43d9`. TypeScript 5.9.3 reported zero parse diagnostics and 3,302,825 AST nodes, including 130,553 string literals and 52,943 function-like nodes with bodies. There were 155,316 syntactic direct call references, 1,149 calls with a first string argument named `tengu_...`, and 792 distinct such labels. There were 1,031 direct `process.env.NAME` references to 483 distinct names, plus 142 other bracket, dynamic, or bare `process.env` references. These counts cover the selected bundle text, not a proven set of application-owned functions or evaluated runtime paths.

| `tengu_` call callee | Calls | Distinct labels | Interpretation |
| --- | ---: | ---: | --- |
| `c` | 980 | 673 | Telemetry event calls |
| `F8` | 121 | 87 | Candidate flag accessor |
| `p5` | 17 | 9 | Candidate flag accessor |
| `oS` | 4 | 4 | Candidate flag accessor |
| Other callees | 27 | 25 | Requires inspection |

The candidate accessor group totals 142 calls and 99 distinct labels after deduplication across its three callees. This is a syntactic grouping, not proof that each label controls a feature. In particular, counting every `tengu_` label as a feature flag would misclassify the 980 `c` telemetry calls. The recorded index lives in the temp directory as `drift-claude-source-index-categories.json`; no AST index or source bytes are committed.

## Limits and useful next steps

The PE structure and full-run hashes are hard byte evidence. The Bun/JSC labels describe nearby literal strings; detailed classification of binary payload records, module boundaries, and bytecode requires a separate container-format parser. A useful next pass would inspect the `.bun` record boundaries around 133121000 and 235931088 and compare record tables to the two identical source ranges. For source map questions, inspect actual references around the five `.rdata` hits before asserting that a map exists. For feature work, narrow by exact marker and verify surrounding code in a private temp extraction, rather than treating a global token count as behavior.
