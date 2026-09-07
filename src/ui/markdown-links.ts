export type MarkdownLink =
  | { kind: "external"; url: string }
  | { kind: "file"; path: string; line?: number; column?: number }
  | { kind: "fragment"; hash: string }
  | { kind: "unsupported" }

const driveAbsolute = /^[a-z]:[/\\]/i
const scheme = /^[a-z][a-z\d+.-]*:/i
const controls = /[\u0000-\u001f\u007f-\u009f]/

function normalizeAbsolutePath(value: string): string | undefined {
  if (!value || controls.test(value)) return
  const windows = driveAbsolute.test(value) || value.startsWith("\\\\") || value.startsWith("//")
  const path = windows ? value.replaceAll("\\", "/") : value
  let root: string
  let rest: string
  if (driveAbsolute.test(path)) {
    root = path.slice(0, 3)
    rest = path.slice(3)
  } else if (path.startsWith("//")) {
    const unc = /^\/\/([^/]+)\/([^/]+)(?:\/(.*))?$/.exec(path)
    if (!unc || [unc[1], unc[2]].some((part) => part === "." || part === "..")) return
    root = `//${unc[1]}/${unc[2]}/`
    rest = unc[3] ?? ""
  } else if (path.startsWith("/")) {
    root = "/"
    rest = path.slice(1)
  } else {
    return
  }
  if (windows && /[<>:"|?*]/.test(path.slice(driveAbsolute.test(path) ? 2 : 0))) return
  const parts: string[] = []
  for (const part of rest.split("/")) {
    if (!part || part === ".") continue
    // Parent traversal stops at the POSIX/drive root or the UNC share, not the server.
    if (part === "..") parts.pop()
    else parts.push(part)
  }
  return root + parts.join("/")
}

/**
 * Classify an un-resolved href, never anchor.href. directory is an absolute native
 * path, not a URL, and is not percent-decoded. File results use / separators on
 * Windows (//Server/Share for UNC); POSIX filename backslashes remain literal.
 * Only roots retain a trailing slash. Dot segments are resolved lexically.
 * Query-bearing file links and encoded separators are deliberately unsupported.
 */
export function classifyMarkdownLink(raw: string, directory?: string): MarkdownLink {
  try {
    if (!raw || raw.trim() !== raw || controls.test(raw)) {
      return { kind: "unsupported" }
    }

    if (/^https?:\/\//i.test(raw) || raw.startsWith("//")) {
      // Do not let URL's permissive slash repair turn malformed hrefs into hosts.
      const external = raw.startsWith("//") ? `https:${raw}` : raw
      if (!/^https?:\/\/[^/\\\s?#]/i.test(external) || external.includes("\\")) return { kind: "unsupported" }
      const url = new URL(external)
      if (url.hostname.toLowerCase().replace(/\.+$/, "") === "tauri.localhost") return { kind: "unsupported" }
      return { kind: "external", url: url.href }
    }

    // Web URLs keep encoded bytes intact; native paths and fragment IDs must decode safely.
    if (controls.test(decodeURIComponent(raw))) return { kind: "unsupported" }
    if (raw.startsWith("#")) return { kind: "fragment", hash: raw }

    const fileURI = /^file:/i.test(raw)
    if (!driveAbsolute.test(raw) && scheme.test(raw) && !fileURI) return { kind: "unsupported" }

    // Split before decoding: %23 is a filename character, not a document fragment.
    const fragmentStart = raw.indexOf("#")
    const target = fragmentStart < 0 ? raw : raw.slice(0, fragmentStart)
    const hash = fragmentStart < 0 ? "" : raw.slice(fragmentStart)
    if (!target || target.includes("?") || /%2f|%5c/i.test(target)) return { kind: "unsupported" }
    let path: string
    if (fileURI) {
      const uri = /^file:\/\/([^/\\]*)(\/[^]*)$/i.exec(target)
      if (!uri || target.includes("\\")) return { kind: "unsupported" }
      // Validate URI syntax without adopting URL's host case-folding or path rewrites.
      new URL(target)
      const host = decodeURIComponent(uri[1])
      path = decodeURIComponent(uri[2])
      if (path.startsWith("//")) return { kind: "unsupported" }
      if (host && host.toLowerCase() !== "localhost") {
        path = `//${host}${path}`
      } else if (/^\/[a-z]:/i.test(path)) {
        path = path.slice(1)
        if (!driveAbsolute.test(path)) return { kind: "unsupported" }
      }
    } else {
      path = decodeURIComponent(target)
      if (!driveAbsolute.test(raw) && scheme.test(path)) return { kind: "unsupported" }
      if (!driveAbsolute.test(path) && !path.startsWith("/") && !path.startsWith("\\\\")) {
        // A single leading backslash is drive-root-relative, not an absolute path.
        if (path.startsWith("\\") || !directory) return { kind: "unsupported" }
        const base = normalizeAbsolutePath(directory)
        if (!base) return { kind: "unsupported" }
        path = `${base.endsWith("/") ? base : `${base}/`}${path}`
      }
    }

    const normalized = normalizeAbsolutePath(path)
    if (!normalized) return { kind: "unsupported" }
    const result: MarkdownLink = { kind: "file", path: normalized }
    const location = /^#L([1-9]\d*)(?:-L([1-9]\d*)|C([1-9]\d*))?$/.exec(hash)
    if (location) {
      const line = Number(location[1])
      const end = location[2] ? Number(location[2]) : line
      const column = location[3] ? Number(location[3]) : undefined
      if (
        Number.isSafeInteger(line) && Number.isSafeInteger(end) && end >= line &&
        (column === undefined || Number.isSafeInteger(column))
      ) {
        result.line = line
        if (column !== undefined) result.column = column
      }
    }
    return result
  } catch {
    return { kind: "unsupported" }
  }
}
