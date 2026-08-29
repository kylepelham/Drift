import { expect, test } from "bun:test"
import { fileLanguageCandidates, resolveFileLanguage } from "../src/syntax-language"

test("file syntax languages come from Linguist metadata and installed Shiki grammars", async () => {
  expect(await fileLanguageCandidates("Containerfile")).toContain("docker")
  expect(await fileLanguageCandidates("views/profile.blade.php")).toEqual(["blade"])
  expect(await fileLanguageCandidates("unknown.very-unlikely-extension")).toEqual(["text"])
})

test("maintained editor metadata settles ambiguous header extensions", async () => {
  const filename = "include/rio_aggregator.h"
  expect(await fileLanguageCandidates(filename)).toEqual(expect.arrayContaining(["c", "cpp", "objective-c"]))
  expect(await resolveFileLanguage(filename)).toBe("cpp")
})

test("maintained editor metadata settles conventional ambiguous extensions", async () => {
  expect(await fileLanguageCandidates("README.md")).toEqual(expect.arrayContaining(["common-lisp", "markdown"]))
  expect(await resolveFileLanguage("README.md")).toBe("markdown")
  expect(await resolveFileLanguage("src/settings.ts")).toBe("typescript")
  expect(await resolveFileLanguage("rtl/counter.v")).toBe("verilog")
  expect(await resolveFileLanguage("native/window.mm")).toBe("objective-cpp")
})

test("ambiguous extensions without a Shiki alias keep a deterministic catalog fallback", async () => {
  expect(await resolveFileLanguage("chart.m")).toBe("matlab")
})
