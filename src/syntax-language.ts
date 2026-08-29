import type { Language } from "linguist-languages"
import type { BundledLanguage } from "shiki"

type SyntaxLanguage = BundledLanguage | "text"
type LanguageMatch = {
  candidates: SyntaxLanguage[]
  preferred: SyntaxLanguage[]
}
type LanguageCatalog = {
  extensions: Map<string, LanguageMatch>
  filenames: Map<string, LanguageMatch>
  shikiNames: Map<string, SyntaxLanguage>
}

let catalogPromise: Promise<LanguageCatalog> | undefined

function normalized(value: string) {
  return value.trim().toLowerCase()
}

function addCandidate(map: Map<string, LanguageMatch>, key: string, language: SyntaxLanguage, preferred: boolean) {
  const current = map.get(key)
  if (!current) return map.set(key, { candidates: [language], preferred: preferred ? [language] : [] })
  if (!current.candidates.includes(language)) current.candidates.push(language)
  if (preferred && !current.preferred.includes(language)) current.preferred.push(language)
}

async function languageCatalog() {
  // A failed import must not be cached: callers fall back to plain text, so a single transient
  // failure would otherwise leave every file unhighlighted until the window reloads.
  return (catalogPromise ??= Promise.all([import("linguist-languages"), import("shiki")])
    .catch((error: unknown) => {
      catalogPromise = undefined
      throw error
    })
    .then(([linguist, shiki]) => {
      const shikiNames = new Map<string, SyntaxLanguage>()
      for (const info of shiki.bundledLanguagesInfo) {
        const language = info.id as SyntaxLanguage
        for (const name of [info.id, info.name, ...(info.aliases ?? [])])
          shikiNames.set(normalized(name), language)
      }

      const extensions = new Map<string, LanguageMatch>()
      const filenames = new Map<string, LanguageMatch>()
      for (const data of Object.values(linguist) as Language[]) {
        const declaredNames = [data.name, ...(data.aliases ?? []), data.group].filter(
          (value): value is string => !!value,
        )
        const editorNames = [data.aceMode, data.codemirrorMode].filter((value): value is string => !!value)
        const declared = declaredNames
          .map((value) => shikiNames.get(normalized(value)))
          .find((value): value is SyntaxLanguage => !!value)
        const editor = editorNames
          .flatMap((value) => [value, ...normalized(value).split(/[_-]/).reverse()])
          .map((value) => shikiNames.get(normalized(value)))
          .find((value): value is SyntaxLanguage => !!value)
        const language = declared ?? editor
        if (!language) continue
        const relatedEditorMode = editorNames.some((mode) =>
          declaredNames.some((name) => {
            const declaredName = normalized(name)
            return declaredName.length >= 3 && declaredName.startsWith(normalized(mode))
          }),
        )
        const preferred = !!declared && (editor === declared || relatedEditorMode)
        for (const extension of data.extensions ?? []) addCandidate(extensions, normalized(extension), language, preferred)
        for (const filename of data.filenames ?? []) addCandidate(filenames, normalized(filename), language, preferred)
      }
      return { extensions, filenames, shikiNames }
    }))
}

export async function fileLanguageCandidates(path: string): Promise<SyntaxLanguage[]> {
  const catalog = await languageCatalog()
  const basename = normalized(path.replaceAll("\\", "/").split("/").at(-1) ?? "")
  if (!basename) return ["text"]

  const filenameMatch = catalog.filenames.get(basename)
  if (filenameMatch) return filenameMatch.candidates

  for (let dot = basename.indexOf("."); dot >= 0; dot = basename.indexOf(".", dot + 1)) {
    const extension = basename.slice(dot)
    const extensionMatch = catalog.extensions.get(extension)
    if (extensionMatch) return extensionMatch.candidates
  }

  const extension = basename.includes(".") ? basename.slice(basename.lastIndexOf(".") + 1) : basename
  return [catalog.shikiNames.get(extension) ?? "text"]
}

export async function resolveFileLanguage(path: string): Promise<SyntaxLanguage> {
  const catalog = await languageCatalog()
  const basename = normalized(path.replaceAll("\\", "/").split("/").at(-1) ?? "")
  if (!basename) return "text"
  const filenameMatch = catalog.filenames.get(basename)
  if (filenameMatch) return filenameMatch.preferred[0] ?? filenameMatch.candidates[0]
  for (let dot = basename.indexOf("."); dot >= 0; dot = basename.indexOf(".", dot + 1)) {
    const extensionMatch = catalog.extensions.get(basename.slice(dot))
    if (extensionMatch) return extensionMatch.preferred[0] ?? extensionMatch.candidates[0]
  }
  const extension = basename.includes(".") ? basename.slice(basename.lastIndexOf(".") + 1) : basename
  return catalog.shikiNames.get(extension) ?? "text"
}
