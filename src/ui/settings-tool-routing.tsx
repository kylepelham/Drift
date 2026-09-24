import { createSignal, onCleanup, onMount, Show } from "solid-js"
import { useEngine } from "../engine"
import { t } from "../state/i18n"
import { listenToolRouting, loadToolRouting, setToolRouting, toolRouting } from "../state/tool-routing"
import { Toggle } from "./controls"
import { SettingsRow } from "./settings-controls"

export function ToolRoutingSetting() {
  const engine = useEngine()
  const [busy, setBusy] = createSignal(true)
  const [error, setError] = createSignal("")
  const report = (cause: unknown) => setError(cause instanceof Error ? cause.message : String(cause))
  onMount(() => {
    let disposed = false
    let stop: (() => void) | undefined
    onCleanup(() => { disposed = true; stop?.() })
    void listenToolRouting()?.then((release) => { if (disposed) release(); else stop = release }).catch(report)
    void loadToolRouting().catch(report).finally(() => setBusy(false))
  })

  async function update(enabled: boolean) {
    if (busy()) return
    setBusy(true)
    setError("")
    try { await setToolRouting(enabled) }
    catch (cause) { report(cause) }
    finally { setBusy(false) }
  }

  return (
    <>
      <SettingsRow title={t("drift.settings.toolRouting.title")} description={t("drift.settings.toolRouting.description")} disabled={busy()}>
        <Toggle label={t("drift.settings.toolRouting.title")} checked={toolRouting().enabled} disabled={busy()} onChange={() => void update(!toolRouting().enabled)} />
      </SettingsRow>
      <Show when={toolRouting().enabled && !engine.state.connected.includes("opencode")}>
        <p class="px-1 py-2 text-xs text-warn">{t("drift.settings.toolRouting.connect")}</p>
      </Show>
      <Show when={error()}><p role="alert" class="px-1 py-2 text-xs text-danger">{error()}</p></Show>
    </>
  )
}
