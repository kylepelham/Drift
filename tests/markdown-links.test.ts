import { describe, expect, test } from "bun:test"
import { classifyMarkdownLink, type MarkdownLink } from "../src/ui/markdown-links"

describe("Markdown file links", () => {
  test.each<[string, string | undefined, string]>([
    ["EAC/docs/BENIGN_PLATFORM_EXPERIMENT_CONTRACT.md", "C:/work", "C:/work/EAC/docs/BENIGN_PLATFORM_EXPERIMENT_CONTRACT.md"],
    ["./docs/../README.md", "C:/work/", "C:/work/README.md"],
    ["../README.md", "C:/work/project", "C:/work/README.md"],
    ["../../../../README.md", "C:/work", "C:/README.md"],
    ["docs//./guide.md", "C:/work/../Work", "C:/Work/docs/guide.md"],
    [".\\docs\\..\\README.md", "c:\\Work", "c:/Work/README.md"],
    ["C:/Work/Docs/../README.md", undefined, "C:/Work/README.md"],
    ["d:\\Work\\Docs\\..\\README.md", "/ignored", "d:/Work/README.md"],
    ["C:/README.md", "not an absolute directory", "C:/README.md"],
    ["/Work/Docs/../README.md", "C:/ignored", "/Work/README.md"],
    ["./Docs/../README.md", "/Work/project", "/Work/project/README.md"],
    ["../../../README.md", "/Work", "/README.md"],
    ["README.md", "/", "/README.md"],
    [".", "C:/Work", "C:/Work"],
    ["/", undefined, "/"],
    ["C:/", undefined, "C:/"],
    ["docs/", "C:/Work", "C:/Work/docs"],
    ["\\\\Server\\Share\\Docs\\..\\README.md", undefined, "//Server/Share/README.md"],
    ["\\\\Server\\Share\\..\\..\\README.md", undefined, "//Server/Share/README.md"],
    ["../README.md", "\\\\Server\\Share\\Docs", "//Server/Share/README.md"],
    ["README.md", "//Server/Share", "//Server/Share/README.md"],
    ["\\\\Server\\Share", undefined, "//Server/Share/"],
    ["file:///C:/Work/Docs/../README.md", undefined, "C:/Work/README.md"],
    ["FILE:///c:/Work/README.md", undefined, "c:/Work/README.md"],
    ["file:///Work/Docs/../README.md", undefined, "/Work/README.md"],
    ["file://localhost/C:/Work/README.md", undefined, "C:/Work/README.md"],
    ["file://LOCALHOST/Work/README.md", undefined, "/Work/README.md"],
    ["file://Server/Share/Docs/../../README.md", undefined, "//Server/Share/README.md"],
    ["My%20Docs/caf%C3%A9%20%E6%96%87.md", "C:/Work", "C:/Work/My Docs/caf\u00e9 \u6587.md"],
    ["caf\u00e9.md", "/Work", "/Work/caf\u00e9.md"],
    ["My Docs/README.md", "/Work", "/Work/My Docs/README.md"],
    ["report%23L42.md", "C:/Work", "C:/Work/report#L42.md"],
    ["file:///C:/Work/report%23part%20one.md", undefined, "C:/Work/report#part one.md"],
    ["file://Server/Share/My%20Docs/caf%C3%A9.md", undefined, "//Server/Share/My Docs/caf\u00e9.md"],
    ["%2e%2e/README.md", "C:/Work/project", "C:/Work/README.md"],
    ["report%2523.md", "C:/Work", "C:/Work/report%23.md"],
    ["a+b.md", "/Work", "/Work/a+b.md"],
    ["a%3Fb.md", "/Work", "/Work/a?b.md"],
    ["docs\\guide.md", "/Work", "/Work/docs\\guide.md"],
    ["README.md", "/Work/100% literal#directory", "/Work/100% literal#directory/README.md"],
    ["./javascript:notes.md", "/Work", "/Work/javascript:notes.md"],
  ])("resolves %s against %s", (raw, directory, path) => {
    expect(classifyMarkdownLink(raw, directory)).toEqual({ kind: "file", path })
  })

  test.each<[string, Partial<Extract<MarkdownLink, { kind: "file" }>>]>([
    ["#L42", { line: 42 }],
    ["#L42-L50", { line: 42 }],
    ["#L42C7", { line: 42, column: 7 }],
    ["#L1-L1", { line: 1 }],
    ["#installation", {}],
    ["#", {}],
    ["#L0", {}],
    ["#L42C0", {}],
    ["#L50-L42", {}],
    ["#L9007199254740992", {}],
    ["#L42C9007199254740992", {}],
    ["#L42-L9007199254740992", {}],
    ["#L42-extra", {}],
    ["#l42", {}],
    ["#%4C42", {}],
  ])("handles document fragment %s", (hash, location) => {
    expect(classifyMarkdownLink(`docs/README.md${hash}`, "C:/Work")).toEqual({
      kind: "file", path: "C:/Work/docs/README.md", ...location,
    })
  })

  test("does not mistake an escaped filename hash for a location", () => {
    expect(classifyMarkdownLink("file:///C:/Work/report%23L42#L7C2")).toEqual({
      kind: "file", path: "C:/Work/report#L42", line: 7, column: 2,
    })
  })
})

describe("Markdown browser links", () => {
  test.each(["#", "#section", "#L42", "#caf%C3%A9", "#one#two"])("preserves %s", (hash) => {
    expect(classifyMarkdownLink(hash)).toEqual({ kind: "fragment", hash })
  })

  test.each([
    ["https://example.com/Guide#L42", "https://example.com/Guide#L42"],
    ["http://example.com", "http://example.com/"],
    ["HTTPS://Example.COM/Docs/../Guide?q=x#part", "https://example.com/Guide?q=x#part"],
    ["//example.com/Guide", "https://example.com/Guide"],
    ["https://example.com/My%20Docs/caf%C3%A9%23part", "https://example.com/My%20Docs/caf%C3%A9%23part"],
    ["https://example.com/My Docs", "https://example.com/My%20Docs"],
    ["https://localhost:8080/Guide", "https://localhost:8080/Guide"],
    ["https://tauri.localhost.example.com/Guide", "https://tauri.localhost.example.com/Guide"],
    ["https://mail.google.com/mail/?view=cm&body=first%0Asecond", "https://mail.google.com/mail/?view=cm&body=first%0Asecond"],
    ["https://example.com/%0a", "https://example.com/%0a"],
    ["https://example.com/?bytes=%FF%00%2F", "https://example.com/?bytes=%FF%00%2F"],
    ["https://example.com/%GG", "https://example.com/%GG"],
    ["//example.com/?body=first%0D%0Asecond", "https://example.com/?body=first%0D%0Asecond"],
  ])("recognizes explicit external URL %s", (raw, url) => {
    expect(classifyMarkdownLink(raw, "C:/Work")).toEqual({ kind: "external", url })
  })

  test.each([
    "http://tauri.localhost/Docs", "https://tauri.localhost/Docs", "//tauri.localhost/Docs",
    "https://TAURI.LOCALHOST:443/Docs", "https://tauri.localhost./Docs",
    "https://tauri%2elocalhost/Docs", "https://user@tauri.localhost/Docs",
    "tauri://localhost/Docs", "TAURI://localhost/Docs",
  ])("blocks internal app origin %s", (raw) => {
    expect(classifyMarkdownLink(raw, "C:/Work")).toEqual({ kind: "unsupported" })
  })
})

describe("unsupported Markdown links", () => {
  test.each([
    "", " ", " README.md", "README.md ",
    "javascript:alert(1)", "JaVaScRiPt:alert(1)", "data:text/html,test", "vbscript:msgbox(1)",
    "mailto:user@example.com", "tel:+123456789", "ftp://example.com/file", "custom:target",
    "C:foo.md", "C:", "C%3A/foo.md", "javascript%3Aalert(1)", "\\README.md",
    "docs/README.md?download=1", "?query", "file:///C:/Docs/README.md?query",
    "file:README.md", "file:C:/README.md", "file://", "file:///C:README.md", "file:////Server/Share/file.md",
    "file://user@Server:80/Share/file.md", "file:///C:\\Work\\README.md",
    "file://user@Server/Share/file.md", "file://Server:80/Share/file.md", "file://[Server]/Share/file.md",
    "file://bad%20host/Share/file.md",
    "\\\\Server", "\\\\Server\\", "\\\\.\\C:\\file.md", "\\\\?\\C:\\file.md",
    "\\\\Server\\..\\file.md", "C:/bad:name.md", "C:/bad*name.md", "C:/bad%3Fname.md",
    "http:example.com", "https:/example.com", "https://", "//", "///example.com",
    "https:///example.com", "https://example.com:invalid", "https://[invalid]/file",
    "https://example.com\\other/file", "//\\example.com/file",
    "bad%.md", "bad%2.md", "bad%GG.md", "bad%C3%28.md", "bad%FF.md",
    "#bad%", "README.md#bad%",
    "docs%2fREADME.md", "docs%5cREADME.md", "file:///Work/docs%2FREADME.md",
    "bad\u0000.md", "bad\n.md", "bad\t.md", "bad\u007f.md", "bad\u0085.md",
    "bad%00.md", "bad%0A.md", "bad%C2%85.md", "#bad%00",
  ])("rejects %s without throwing", (raw) => {
    expect(classifyMarkdownLink(raw, "C:/Work")).toEqual({ kind: "unsupported" })
  })

  test.each([undefined, "", "Work", "C:Work", "https://tauri.localhost/", "file:///C:/Work", "C:/bad\npath"])(
    "does not resolve a relative path with invalid context %s", (directory) => {
      expect(classifyMarkdownLink("EAC/docs/BENIGN_PLATFORM_EXPERIMENT_CONTRACT.md", directory)).toEqual({ kind: "unsupported" })
    },
  )
})
