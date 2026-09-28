import { createSignal, onCleanup, onMount, Show } from "solid-js"
import { t } from "../state/i18n"
import {
  listenToolRouting, loadToolRouting, loadToolRoutingStatus, setToolRouting, toolRouting, toolRoutingStatus,
  type ToolRoutingStatus,
} from "../state/tool-routing"
import { Toggle } from "./controls"
import { SettingsRow } from "./settings-controls"

const outcomes = new Set([
  "routed", "no-key", "unauthorized", "insufficient-funds", "http-error", "timeout", "network",
  "invalid-response", "uncertain", "no-context", "too-few-groups", "catalog-too-large",
])

function statusText(status: ToolRoutingStatus) {
  if (!outcomes.has(status.outcome)) return ""
  return t(`drift.settings.toolRouting.outcome.${status.outcome}`, { count: status.hidden ?? 0, status: status.httpStatus ?? "" })
}

export function ToolRoutingSetting() {
  const [busy, setBusy] = createSignal(true)
  const [error, setError] = createSignal("")
  const report = (cause: unknown) => setError(cause instanceof Error ? cause.message : String(cause))
  onMount(() => {
    let disposed = false
    let stop: (() => void) | undefined
    const poll = setInterval(() => void loadToolRoutingStatus().catch(() => undefined), 4000)
    onCleanup(() => { disposed = true; stop?.(); clearInterval(poll) })
    void listenToolRouting()?.then((release) => { if (disposed) release(); else stop = release }).catch(report)
    void loadToolRouting().then(loadToolRoutingStatus).catch(report).finally(() => setBusy(false))
  })

  async function update(enabled: boolean) {
    if (busy()) return
    setBusy(true)
    setError("")
    try { await setToolRouting(enabled) }
    catch (cause) { report(cause) }
    finally { setBusy(false) }
  }

  const status = () => (toolRouting().enabled && toolRoutingStatus()) || null

  return (
    <>
      <SettingsRow title={t("drift.settings.toolRouting.title")} description={t("drift.settings.toolRouting.description")} disabled={busy()}>
        <Toggle label={t("drift.settings.toolRouting.title")} checked={toolRouting().enabled} disabled={busy()} onChange={() => void update(!toolRouting().enabled)} />
      </SettingsRow>
      <Show when={status()}>
        {(current) => (
          <p role="status" class="px-1 py-2 text-xs" classList={{ "text-ink-faint": current().outcome === "routed", "text-warn": current().outcome !== "routed" }}>
            {statusText(current())}
          </p>
        )}
      </Show>
      <Show when={error()}><p role="alert" class="px-1 py-2 text-xs text-danger">{error()}</p></Show>
    </>
  )
}
