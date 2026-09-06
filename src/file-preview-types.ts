export const filePreviewTypes = ["markdown", "pdf", "image", "text", "table", "audio", "video"] as const
export type FilePreviewType = (typeof filePreviewTypes)[number]

export const filePreviewLimits: Record<FilePreviewType, number> = {
  markdown: 2 * 1024 ** 2,
  pdf: 20 * 1024 ** 2,
  image: 10 * 1024 ** 2,
  text: 2 * 1024 ** 2,
  table: 5 * 1024 ** 2,
  audio: 20 * 1024 ** 2,
  video: 40 * 1024 ** 2,
}

const extensions: Record<FilePreviewType, string> = {
  markdown: "md markdown mdown",
  pdf: "pdf",
  image: "png jpg jpeg jfif gif webp avif bmp ico svg apng",
  text: "txt text log json jsonc jsonl ndjson yaml yml toml ini cfg conf config xml html htm xhtml css scss sass less js jsx mjs cjs ts tsx mts cts vue svelte astro c h cc cpp cxx hpp hxx cs fs fsx vb java kt kts scala sc go rs py pyi pyw rb erb php phtml swift m mm sh bash zsh fish ps1 psm1 psd1 bat cmd lua pl pm r rmd sql graphql gql proto prisma ex exs erl hrl clj cljs cljc edn lisp el hs lhs ml mli nim zig v sv vhd vhdl tex bib rst adoc asciidoc org diff patch properties gradle cmake make mk dockerfile gitignore gitattributes gitmodules editorconfig npmrc nvmrc lock srt vtt ipynb",
  table: "csv tsv",
  audio: "mp3 wav ogg oga m4a aac flac opus aif aiff",
  video: "mp4 m4v webm ogv mov",
}

const byExtension = new Map(
  filePreviewTypes.flatMap((type) => extensions[type].split(" ").map((extension) => [extension, type] as const)),
)
const textNames = new Set([
  "readme", "license", "licence", "copying", "authors", "changelog", "notice",
  "makefile", "gnumakefile", "dockerfile", "containerfile", "gemfile", "rakefile", "procfile", "justfile",
  ".env", ".gitignore", ".gitattributes", ".gitmodules", ".gitconfig", ".gitkeep",
  ".dockerignore", ".editorconfig", ".npmrc", ".yarnrc", ".nvmrc", ".node-version", ".python-version",
  ".bashrc", ".bash_profile", ".bash_logout", ".zshrc", ".zprofile", ".profile",
  ".prettierrc", ".prettierignore", ".eslintrc", ".eslintignore", ".browserslistrc",
])

function filename(path: string) {
  return path.split(/[\\/]/).pop()!.toLowerCase()
}

/** Filename classification only, not content validation or permission to read the path. */
export function filePreviewType(path: string): FilePreviewType | undefined {
  const name = filename(path)
  if (textNames.has(name) || /^\.env\.(?:local|development|production|test|staging|example|sample)(?:\.local)?$/.test(name))
    return "text"
  const dot = name.lastIndexOf(".")
  return dot < 0 ? undefined : byExtension.get(name.slice(dot + 1))
}

const mimeByExtension = new Map(Object.entries({
  pdf: "application/pdf",
  png: "image/png", apng: "image/apng", jpg: "image/jpeg", jpeg: "image/jpeg", jfif: "image/jpeg",
  gif: "image/gif", webp: "image/webp", avif: "image/avif", bmp: "image/bmp", ico: "image/x-icon",
  svg: "image/svg+xml",
  csv: "text/csv", tsv: "text/tab-separated-values",
  mp3: "audio/mpeg", wav: "audio/wav", ogg: "audio/ogg", oga: "audio/ogg", opus: "audio/ogg",
  m4a: "audio/mp4", aac: "audio/aac", flac: "audio/flac", aif: "audio/aiff", aiff: "audio/aiff",
  mp4: "video/mp4", m4v: "video/mp4", webm: "video/webm", ogv: "video/ogg", mov: "video/quicktime",
}))

/** HTML/code stay plain text. SVG blobs must only be rendered in img, never an executable document. */
export function filePreviewMime(path: string): string {
  const type = filePreviewType(path)
  if (!type) return "application/octet-stream"
  if (type === "text") return "text/plain"
  if (type === "markdown") return "text/markdown"
  return mimeByExtension.get(filename(path).split(".").pop()!) ?? "application/octet-stream"
}
