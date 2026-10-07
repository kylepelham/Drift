import { createSignal, For, onMount, Show } from "solid-js"
import { useEngine } from "../engine"
import type { PluginInfo } from "../engine/native/client"
import { t } from "../state/i18n"
import { SettingsGroup } from "./settings-controls"

const configFile = "~/.config/drift/drift.json"

/** The engine's plugins as it loaded them, with a Reload that reads drift.json again. */
export function PluginsSection() {
  const engine = useEngine()
  const [plugins, setPlugins] = createSignal<PluginInfo[]>([])
  const [loading, setLoading] = createSignal(false)
  const [failure, setFailure] = createSignal("")

  const run = async (action: () => Promise<PluginInfo[]>) => {
    setLoading(true)
    setFailure("")
    try {
      setPlugins(await action())
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error))
    } finally {
      setLoading(false)
    }
  }
  onMount(() => void run(() => engine.actions.plugins()))

  const reload = (
    <button
      class="rounded-md border border-edge px-3 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
      disabled={loading() || engine.state.connection !== "online"}
      onClick={() => void run(() => engine.actions.reloadPlugins())}
    >
      {t("drift.plugins.reload")}
    </button>
  )

  return (
    <SettingsGroup title={t("drift.settings.plugins")} action={reload}>
      <Show when={failure()}>
        <div role="alert" class="px-1 py-2.5 text-xs text-danger">{failure()}</div>
      </Show>
      <Show when={loading() && !plugins().length}>
        <div role="status" class="px-1 py-3 text-xs text-ink-faint">{t("drift.plugins.loading")}</div>
      </Show>
      <For each={plugins()}>{(plugin) => <PluginRow plugin={plugin} />}</For>
      <Show when={!loading() && !failure() && !plugins().length}>
        <div class="px-1 py-3 text-xs text-ink-faint">{t("drift.plugins.empty", { path: configFile })}</div>
      </Show>
      <Show when={plugins().length}>
        <div class="px-1 py-2.5 text-[0.72rem] text-ink-faint">{t("drift.plugins.file", { path: configFile })}</div>
      </Show>
    </SettingsGroup>
  )
}

function PluginRow(props: { plugin: PluginInfo }) {
  return (
    <div class="flex min-h-13 items-center gap-4 border-b border-edge/70 px-1 py-2.5">
      <div class="min-w-0 flex-1">
        <div class="truncate text-[0.82rem] font-medium text-ink">{props.plugin.name}</div>
        <div class="mt-0.5 truncate font-mono text-[0.72rem] text-ink-faint">{props.plugin.path}</div>
      </div>
      <div class="max-w-[55%] truncate text-xs" classList={{ "text-ok": !props.plugin.error, "text-danger": !!props.plugin.error }} title={props.plugin.error ?? undefined}>
        {props.plugin.error ?? t("drift.plugins.loaded")}
      </div>
    </div>
  )
}
