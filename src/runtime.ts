export type RuntimeLocation = Pick<Location, "pathname" | "origin">

export function remoteRuntimeFrom(location: RuntimeLocation) {
  return location.pathname === "/companion" || location.pathname.startsWith("/companion/")
}

export function isRemoteRuntime() {
  return typeof window !== "undefined" && remoteRuntimeFrom(window.location)
}

export function remoteEngineBase(location?: RuntimeLocation) {
  const current = location ?? (typeof window !== "undefined" ? window.location : undefined)
  return current && remoteRuntimeFrom(current) ? `${current.origin}/engine` : undefined
}
