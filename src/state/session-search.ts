/**
 * Finding a previous conversation, by title or by something said inside it.
 *
 * Titles are already in memory, so those are matched here. Message bodies are not: a workspace
 * holds far more transcript than the cache ever loads, so content search is answered by the
 * desktop backend against the engine database instead of pulling every transcript first.
 */

import type { DriftStore, SessionContentMatch } from "./store"

export type SessionSearchMode = "name" | "content"

export type SessionSearchHit = {
  sessionId: string
  title: string
  workspaceId: string
  workspaceName: string
  updatedAt: number
  archived: boolean
  /** Content hits carry the message that matched, so opening the result can land on it. */
  messageId?: string
  excerpt?: string
}

export type SearchableSession = {
  id: string
  title: string
  directory: string
  updatedAt: number
}

export type SearchableWorkspace = { id: string; name: string; path: string }

/** Keystrokes settle before a query is issued; content search reaches the database. */
export const sessionSearchDebounceMs = 180
/** Shorter than this matches too much to be worth showing. */
export const minSessionSearchChars = 2
const maxNameResults = 40

function normalizeDirectory(path: string) {
  return path.replaceAll("\\", "/").replace(/\/+$/, "").toLowerCase()
}

export function sessionSearchReady(query: string) {
  return query.trim().length >= minSessionSearchChars
}

/**
 * Ranks title matches so the most literal ones lead: a title that starts with the query, then one
 * that contains it as a whole word, then any substring. Recency breaks ties.
 */
export function rankTitleMatch(title: string, query: string) {
  const haystack = title.toLowerCase()
  const needle = query.trim().toLowerCase()
  if (!needle || !haystack.includes(needle)) return -1
  if (haystack.startsWith(needle)) return 0
  return new RegExp(`\\b${needle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`).test(haystack) ? 1 : 2
}

export function searchSessionNames(
  sessions: SearchableSession[],
  workspaces: SearchableWorkspace[],
  archived: ReadonlySet<string>,
  query: string,
): SessionSearchHit[] {
  if (!sessionSearchReady(query)) return []
  const byDirectory = new Map(workspaces.map((workspace) => [normalizeDirectory(workspace.path), workspace]))
  return sessions
    .map((session) => ({ session, rank: rankTitleMatch(session.title, query) }))
    .filter((entry) => entry.rank >= 0)
    .sort((a, b) => a.rank - b.rank || b.session.updatedAt - a.session.updatedAt)
    .slice(0, maxNameResults)
    .map(({ session }) => {
      const workspace = byDirectory.get(normalizeDirectory(session.directory))
      return {
        sessionId: session.id,
        title: session.title,
        workspaceId: workspace?.id ?? "",
        workspaceName: workspace?.name ?? session.directory,
        updatedAt: session.updatedAt,
        archived: archived.has(session.id),
      }
    })
}

/** Joins backend content matches to the workspace they belong to, dropping unknown workspaces. */
export function contentHits(
  matches: SessionContentMatch[],
  workspaces: SearchableWorkspace[],
  archived: ReadonlySet<string>,
): SessionSearchHit[] {
  const byDirectory = new Map(workspaces.map((workspace) => [normalizeDirectory(workspace.path), workspace]))
  return matches.map((match) => {
    const workspace = byDirectory.get(normalizeDirectory(match.directory))
    return {
      sessionId: match.sessionId,
      title: match.title,
      workspaceId: workspace?.id ?? "",
      workspaceName: workspace?.name ?? match.directory,
      updatedAt: match.updatedAt,
      archived: archived.has(match.sessionId),
      messageId: match.messageId || undefined,
      excerpt: match.excerpt,
    }
  })
}

/**
 * Splits `text` around every case-insensitive occurrence of `query`, so the view can mark matches
 * without building HTML from user content.
 */
export function highlightSegments(text: string, query: string) {
  const needle = query.trim().toLowerCase()
  if (!needle) return [{ text, match: false }]
  const segments: { text: string; match: boolean }[] = []
  const haystack = text.toLowerCase()
  let cursor = 0
  for (let at = haystack.indexOf(needle); at !== -1; at = haystack.indexOf(needle, cursor)) {
    if (at > cursor) segments.push({ text: text.slice(cursor, at), match: false })
    segments.push({ text: text.slice(at, at + needle.length), match: true })
    cursor = at + needle.length
  }
  if (cursor < text.length) segments.push({ text: text.slice(cursor), match: false })
  return segments
}

export type SessionSearchDependencies = {
  store: Pick<DriftStore, "searchSessions">
  sessions: () => SearchableSession[]
  workspaces: () => SearchableWorkspace[]
  archived: () => ReadonlySet<string>
}

export type SessionSearchState = {
  query: string
  mode: SessionSearchMode
  hits: SessionSearchHit[]
  loading: boolean
  error: string
}

/**
 * Runs one search at a time and keeps only the newest answer.
 *
 * Content queries are asynchronous and can overtake each other, so every run carries a sequence
 * number and a late reply for a superseded query is dropped rather than shown.
 */
export function createSessionSearchRunner(dependencies: SessionSearchDependencies) {
  let sequence = 0
  return async function run(
    query: string,
    mode: SessionSearchMode,
    apply: (state: Partial<SessionSearchState>) => void,
  ) {
    const current = ++sequence
    if (!sessionSearchReady(query)) {
      apply({ hits: [], loading: false, error: "" })
      return
    }
    if (mode === "name") {
      const hits = searchSessionNames(
        dependencies.sessions(),
        dependencies.workspaces(),
        dependencies.archived(),
        query,
      )
      if (current === sequence) apply({ hits, loading: false, error: "" })
      return
    }
    apply({ loading: true, error: "" })
    try {
      const matches = await dependencies.store.searchSessions(query, "")
      if (current !== sequence) return
      apply({ hits: contentHits(matches, dependencies.workspaces(), dependencies.archived()), loading: false })
    } catch (error) {
      if (current !== sequence) return
      apply({ hits: [], loading: false, error: error instanceof Error ? error.message : String(error) })
    }
  }
}
