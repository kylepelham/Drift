import { persisted } from "./persist"
import { isRemoteRuntime } from "../runtime"
import { parseNavigationHash, pushRemoteSelection } from "./navigation"
import { currentMirrorSnapshot, publishMirrorSelection } from "./mirror"

const [selectedSession, setSelectedSession] = persisted<string | null>("drift.session", null)
export { selectedSession }

export function selectSession(session: string | null, navigate = true) {
  setSelectedSession(session)
  const workspaceId = currentMirrorSnapshot()?.selection.workspaceId ?? null
  publishMirrorSelection({ workspaceId, sessionId: session })
  if (navigate) pushRemoteSelection({ session: session ?? undefined })
}

export function applyMirroredSession(session: string | null) {
  setSelectedSession(session)
}

if (typeof window !== "undefined" && isRemoteRuntime()) {
  window.addEventListener("popstate", () => setSelectedSession(parseNavigationHash(window.location.hash).session ?? null))
}
