import { persisted } from "./persist"
import { isRemoteRuntime } from "../runtime"
import { parseNavigationHash, pushRemoteSelection } from "./navigation"

const [selectedSession, setSelectedSession] = persisted<string | null>("drift.session", null)
export { selectedSession }

export function selectSession(session: string | null, navigate = true) {
  setSelectedSession(session)
  if (navigate) pushRemoteSelection({ session: session ?? undefined })
}

if (typeof window !== "undefined" && isRemoteRuntime()) {
  window.addEventListener("popstate", () => setSelectedSession(parseNavigationHash(window.location.hash).session ?? null))
}
