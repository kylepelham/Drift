import { expect, mock, test } from "bun:test"
import * as ts from "typescript"
import { classifyMarkdownLink } from "../src/ui/markdown-links"

const source = await Bun.file(new URL("../src/ui/html-preview.tsx", import.meta.url)).text()
const parsed = ts.createSourceFile("html-preview.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
const builder = parsed.statements.find((node): node is ts.FunctionDeclaration => ts.isFunctionDeclaration(node) && node.name?.text === "htmlPreviewDocument")!
const compile = new Function("createDOMPurify", "window", "classifyMarkdownLink", "markdownImageAttribute", `${ts.transpileModule(builder.getText(parsed).replace(/^export /, ""), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None },
}).outputText}; return htmlPreviewDocument`)

function setup(sources: string[] = []) {
  let hook!: (node: { nodeName: string; namespaceURI: string }, attribute: { attrName: string; attrValue: string; keepAttr: boolean }) => void
  const images = sources.map((src) => ({
    nodeName: "IMG", namespaceURI: "http://www.w3.org/1999/xhtml", src,
    setAttribute: mock((_name: string, _value: string) => {}),
  }))
  const nodes: any[] = []
  const head = { removeAttribute: mock(() => {}), prepend: mock(() => {}) }
  const html = {
    outerHTML: "<html><head></head><body>sanitized</body></html>",
    querySelector: mock(() => head), removeAttribute: mock(() => {}),
    querySelectorAll: mock(() => images),
    ownerDocument: { createElement: (name: string) => { const node = { name, after: mock(() => {}) }; nodes.push(node); return node } },
  }
  const sanitize = mock((_text: string, _config: any) => {
    for (const image of images) hook(image, { attrName: "src", attrValue: image.src, keepAttr: true })
    return html
  })
  const purify = { sanitize, addHook: (name: string, callback: typeof hook) => { expect(name).toBe("uponSanitizeAttribute"); hook = callback } }
  const window = {}
  const factory = mock((_window: unknown) => purify)
  const render = compile(factory, window, classifyMarkdownLink, "data-document-image")
  return { render, factory, window, sanitize, html, head, nodes, images, allowed: (nodeName: string, attrName: string, attrValue: string) => {
    const attribute = { attrName, attrValue, keepAttr: true }
    hook({ nodeName, namespaceURI: "http://www.w3.org/1999/xhtml" }, attribute)
    return attribute.keepAttr
  } }
}

test("HTML preview uses its own sanitizer and serializes only the sanitized whole document", () => {
  const h = setup()
  const original = '<script>parent.pwned = true</script><h1>Plan</h1>'
  expect(h.render(original)).toBe(`<!doctype html>\n${h.html.outerHTML}`)
  expect(h.factory).toHaveBeenCalledWith(h.window)
  expect(h.sanitize.mock.calls[0][0]).toBe(original)
  const config = h.sanitize.mock.calls[0][1]
  expect(config.WHOLE_DOCUMENT).toBeTrue()
  expect(config.RETURN_DOM).toBeTrue()
  for (const tag of ["script", "iframe", "object", "embed", "base", "link", "meta", "form", "input", "animate", "set"]) expect(config.FORBID_TAGS).toContain(tag)
  for (const attr of ["href", "xlink:href", "srcset", "srcdoc", "action", "formaction", "target", "ping", "autofocus", "data-document-image"]) expect(config.FORBID_ATTR).toContain(attr)
  expect(config.FORBID_TAGS).not.toContain("style")
})

test("HTML CSP precedes document styles and disallows network resources and script execution", () => {
  const h = setup()
  h.render("<h1>Plan</h1>")
  const [policy, defaults] = h.nodes
  expect(policy.name).toBe("meta")
  expect(policy.httpEquiv).toBe("Content-Security-Policy")
  expect(policy.content).toBe("default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; base-uri 'none'; form-action 'none'")
  expect(h.head.prepend).toHaveBeenCalledWith(policy)
  expect(policy.after).toHaveBeenCalledWith(defaults)
  expect(h.html.removeAttribute).toHaveBeenCalledWith("style")
  expect(h.head.removeAttribute).toHaveBeenCalledWith("style")
})

test("HTML source attributes allow embedded images but never directly load local or external resources", () => {
  const h = setup()
  h.render("")
  for (const src of ["local.png", "/local.png", "file:///C:/work/a.png", "https://example.test/a.png", "//example.test/a.png", "blob:caller-owned", "data:text/html,<script>alert(1)</script>", "data:image/png.evil,x", "javascript:alert(1)"]) {
    expect(h.allowed("IMG", "src", src), src).toBeFalse()
  }
  for (const src of ["data:image/png;base64,abc", "data:image/svg+xml,%3Csvg%3E", "DATA:image/webp;base64,abc"]) {
    expect(h.allowed("IMG", "src", src), src).toBeTrue()
    expect(h.allowed("IFRAME", "src", src)).toBeFalse()
    expect(h.allowed("SCRIPT", "src", src)).toBeFalse()
  }
  expect(h.allowed("P", "style", "color:red")).toBeTrue()
})

test("HTML local image sources become trusted markers without resolving against the app URL", () => {
  const locals = ["assets/diagram.png", "../shared/a%20b.svg", "C:\\work\\image.jpg", "file:///C:/work/image.png", "/workspace/image.png"]
  const blocked = ["https://example.test/a.png", "//example.test/a.png", "file://server/share/a.png", "\\\\server\\share\\a.png", "blob:forged", "javascript:alert(1)", "data:image/png;base64,abc", "a%00.png", "a%2fb.png"]
  const h = setup([...locals, ...blocked])
  h.render("")
  for (const [index, raw] of locals.entries()) expect(h.images[index].setAttribute).toHaveBeenCalledWith("data-document-image", raw)
  for (const image of h.images.slice(locals.length)) expect(image.setAttribute).not.toHaveBeenCalled()
})

test("HTML opens rendered first with keyboard tabs, original source, and a scriptless iframe", async () => {
  const host = await Bun.file(new URL("../src/ui/file-preview.tsx", import.meta.url)).text()
  expect(host.indexOf('file().kind === "text" && /\\.html?$/i.test(props.file.path)')).toBeGreaterThan(0)
  expect(host.indexOf("<HtmlPreview")).toBeLessThan(host.indexOf("<ProgressiveCodeView"))
  expect(source).toContain("createSignal(false)")
  expect(source).toContain('role="tablist"')
  expect(source).toContain('role="tab" aria-selected={source() === value}')
  expect(source).toContain('role="tabpanel"')
  expect(source).toContain('["ArrowLeft", "ArrowRight", "Home", "End"]')
  expect(source).toContain("<ProgressiveCodeView code={props.text}")
  expect(source).toContain('sandbox="allow-same-origin" referrerPolicy="no-referrer" srcdoc={html}')
  expect(source).not.toContain("allow-scripts")
  expect(source).not.toContain("innerHTML")
  expect(source).toContain('content.removeEventListener("keydown", escape)')
  expect(source).toContain("onCleanup(() => detach?.())")
  expect(host).toContain("path={props.file.path} directory={props.file.directory}")
  expect(source).toContain('<Show when={!source() && document()} keyed')
  expect(source).toContain("onCleanup(observeMarkdownImages(body")
  expect(source).toContain("parent: previewParentDirectory(props.path)")
  expect(source).toContain("directory: props.directory")
  expect(source).toContain('enabled: shouldPreviewFile("image.png")')
  expect(source).toContain("interactive: false")
})
