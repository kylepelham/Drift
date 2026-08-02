import { createSignal } from "solid-js"
import { isRemoteRuntime } from "../runtime"

export const mobileBreakpoint = 720
export function isNarrowWidth(width: number) {
  return width < mobileBreakpoint
}

export type RemoteRoute = { workspace?: string; session?: string; overlay?: "drawer" | "settings" }

export function navigationHash(route: RemoteRoute) {
  const params = new URLSearchParams()
  if (route.workspace) params.set("workspace", route.workspace)
  if (route.session) params.set("session", route.session)
  if (route.overlay) params.set("overlay", route.overlay)
  return `#/${params.toString()}`
}

export function parseNavigationHash(hash: string): RemoteRoute {
  const params = new URLSearchParams(hash.replace(/^#\/?/, ""))
  const overlay = params.get("overlay")
  return {
    workspace: params.get("workspace") || undefined,
    session: params.get("session") || undefined,
    overlay: overlay === "drawer" || overlay === "settings" ? overlay : undefined,
  }
}

function currentRoute() {
  return parseNavigationHash(window.location.hash)
}

export function pushRemoteSelection(patch: Pick<RemoteRoute, "workspace" | "session">) {
  if (!isRemoteRuntime()) return
  const next = { ...currentRoute(), ...patch, overlay: undefined }
  history.pushState(null, "", navigationHash(next))
}

export function replaceRemoteSelection(workspace: string | null, session: string | null) {
  if (!isRemoteRuntime()) return
  history.replaceState(null, "", navigationHash({ workspace: workspace ?? undefined, session: session ?? undefined }))
}

export function pushRemoteOverlay(overlay: RemoteRoute["overlay"]) {
  if (!isRemoteRuntime()) return
  history.pushState(null, "", navigationHash({ ...currentRoute(), overlay }))
}

const [mobileDrawerOpen, setMobileDrawerOpen] = createSignal(false)
export { mobileDrawerOpen }

export function openMobileDrawer() {
  setMobileDrawerOpen(true)
  pushRemoteOverlay("drawer")
}

export function closeMobileDrawer(fromHistory = false) {
  setMobileDrawerOpen(false)
  if (!fromHistory && isRemoteRuntime() && currentRoute().overlay === "drawer") history.back()
}

if (typeof window !== "undefined") {
  window.addEventListener("popstate", () => setMobileDrawerOpen(currentRoute().overlay === "drawer"))
}
