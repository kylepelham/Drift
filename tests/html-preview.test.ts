import { expect, mock, test } from "bun:test"
import * as ts from "typescript"

const source = await Bun.file(new URL("../src/ui/html-preview.tsx", import.meta.url)).text()
const parsed = ts.createSourceFile("html-preview.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
const builder = parsed.statements.find((node): node is ts.FunctionDeclaration => ts.isFunctionDeclaration(node) && node.name?.text === "htmlPreviewDocument")!
const compile = new Function("createDOMPurify", "window", `${ts.transpileModule(builder.getText(parsed).replace(/^export /, ""), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None },
}).outputText}; return htmlPreviewDocument`)

function setup() {
  let hook!: (node: { nodeName: string }, attribute: { attrName: string; attrValue: string; keepAttr: boolean }) => void
  const nodes: any[] = []
  const head = { removeAttribute: mock(() => {}), prepend: mock(() => {}) }
  const html = {
    outerHTML: "<html><head></head><body>sanitized</body></html>",
    querySelector: mock(() => head), removeAttribute: mock(() => {}),
    ownerDocument: { createElement: (name: string) => { const node = { name, after: mock(() => {}) }; nodes.push(node); return node } },
  }
  const sanitize = mock((_text: string, _config: any) => html)
  const purify = { sanitize, addHook: (name: string, callback: typeof hook) => { expect(name).toBe("uponSanitizeAttribute"); hook = callback } }
  const window = {}
  const factory = mock((_window: unknown) => purify)
  const render = compile(factory, window)
  return { render, factory, window, sanitize, html, head, nodes, allowed: (nodeName: string, attrName: string, attrValue: string) => {
    const attribute = { attrName, attrValue, keepAttr: true }
    hook({ nodeName }, attribute)
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
  for (const attr of ["href", "xlink:href", "srcset", "srcdoc", "action", "formaction", "target", "ping", "autofocus"]) expect(config.FORBID_ATTR).toContain(attr)
  expect(config.FORBID_TAGS).not.toContain("style")
})

test("HTML CSP precedes document styles and disallows network resources and script execution", () => {
  const h = setup()
  h.render("<h1>Plan</h1>")
  const [policy, defaults] = h.nodes
  expect(policy.name).toBe("meta")
  expect(policy.httpEquiv).toBe("Content-Security-Policy")
  expect(policy.content).toBe("default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; base-uri 'none'; form-action 'none'")
  expect(h.head.prepend).toHaveBeenCalledWith(policy)
  expect(policy.after).toHaveBeenCalledWith(defaults)
  expect(h.html.removeAttribute).toHaveBeenCalledWith("style")
  expect(h.head.removeAttribute).toHaveBeenCalledWith("style")
})

test("HTML source attributes allow only embedded images, never local or external resources", () => {
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
  expect(source).toContain('sandbox="allow-same-origin" referrerPolicy="no-referrer" srcdoc={document()}')
  expect(source).not.toContain("allow-scripts")
  expect(source).not.toContain("innerHTML")
  expect(source).toContain('content.removeEventListener("keydown", escape)')
  expect(source).toContain("onCleanup(() => detach?.())")
})
