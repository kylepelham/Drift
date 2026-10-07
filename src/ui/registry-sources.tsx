import { createSignal, For, onMount, Show } from "solid-js"
import { useEngine } from "../engine"
import { t } from "../state/i18n"
import { loadRegistrySources, registrySources, saveRegistrySources, validSourceUrl, type RegistryKind, type RegistrySource } from "../state/registry-sources"
import { IconArrowUp, IconPlus, IconTrash } from "./icons"

/** Registries of one kind the user added, with a row to add another; shared by the plugin and MCP pages. */
export function RegistrySourcesSheet(props: { kind: RegistryKind; onBack: () => void }) {
  const engine = useEngine()
  const [name, setName] = createSignal("")
  const [url, setUrl] = createSignal("")
  const [busy, setBusy] = createSignal(false)
  const [error, setError] = createSignal("")
  const client = () => ({ settings: () => engine.actions.engineSettings(), putSettings: (body: { registrySources: RegistrySource[] }) => engine.actions.putEngineSettings(body) })
  onMount(() => void loadRegistrySources(client()).catch(() => setError(t("drift.registry.sources.loadFailed"))))
  const mine = () => registrySources().filter((source) => source.kind === props.kind)
  const canAdd = () => name().trim().length > 0 && validSourceUrl(url().trim()) && !busy()
  const save = async (next: RegistrySource[]) => {
    setBusy(true)
    setError("")
    try {
      await saveRegistrySources(client(), next)
      return true
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      return false
    } finally {
      setBusy(false)
    }
  }
  const add = async () => {
    const source: RegistrySource = { name: name().trim(), url: url().trim(), kind: props.kind }
    if (await save([...registrySources().filter((item) => item.url !== source.url), source])) {
      setName("")
      setUrl("")
    }
  }
  const remove = (source: RegistrySource) => void save(registrySources().filter((item) => item !== source))

  return (
    <div class="space-y-4">
      <button class="flex items-center gap-1.5 text-xs text-ink-faint hover:text-ink" onClick={props.onBack}>
        <IconArrowUp class="size-3.5 -rotate-90" />
        {t("drift.mcp.registry.back")}
      </button>
      <div>
        <div class="text-base font-semibold text-ink">{t("drift.registry.sources")}</div>
        <div class="mt-1 text-sm text-ink-muted">{t(props.kind === "plugins" ? "drift.registry.sources.pluginsDescription" : "drift.registry.sources.mcpDescription")}</div>
      </div>
      <Show when={error()}>
        <div role="alert" class="rounded-md border border-danger/35 bg-danger/10 px-3 py-2 text-xs text-danger">{error()}</div>
      </Show>
      <div class="border-y border-edge/80">
        <For each={mine()}>
          {(source) => (
            <div class="flex items-center gap-3 border-b border-edge/70 px-3 py-2.5 last:border-b-0">
              <div class="min-w-0 flex-1">
                <div class="truncate text-sm font-medium text-ink">{source.name}</div>
                <div class="truncate font-mono text-xs text-ink-faint">{source.url}</div>
              </div>
              <button
                type="button"
                title={t("drift.registry.sources.remove")}
                aria-label={t("drift.registry.sources.remove")}
                class="flex size-7 shrink-0 items-center justify-center rounded-md border border-danger/40 text-danger hover:bg-danger/10 disabled:opacity-40"
                disabled={busy()}
                onClick={() => remove(source)}
              >
                <IconTrash class="size-3.5" />
              </button>
            </div>
          )}
        </For>
        <Show when={!mine().length}>
          <div class="px-3 py-4 text-sm text-ink-faint">{t("drift.registry.sources.empty")}</div>
        </Show>
      </div>
      <div class="space-y-2 rounded-lg border border-edge bg-surface p-3">
        <div class="text-xs font-medium text-ink">{t("drift.registry.sources.add")}</div>
        <div class="flex flex-col gap-2 sm:flex-row">
          <input
            aria-label={t("drift.registry.sources.name")}
            placeholder={t("drift.registry.sources.name")}
            class="h-8 min-w-0 rounded-md border border-edge bg-raised/45 px-2.5 text-xs text-ink outline-none placeholder:text-ink-faint focus:border-accent sm:w-44"
            value={name()}
            onInput={(event) => setName(event.currentTarget.value)}
          />
          <input
            aria-label={t("drift.registry.sources.url")}
            placeholder="https://registry.example.com/plugins.json"
            spellcheck={false}
            class="h-8 min-w-0 flex-1 rounded-md border border-edge bg-raised/45 px-2.5 font-mono text-xs text-ink outline-none placeholder:text-ink-faint focus:border-accent"
            value={url()}
            onInput={(event) => setUrl(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && canAdd()) void add()
            }}
          />
          <button
            class="flex h-8 shrink-0 items-center justify-center gap-1.5 rounded-md bg-accent px-3 text-xs font-medium text-accent-ink disabled:opacity-40"
            disabled={!canAdd()}
            onClick={() => void add()}
          >
            <IconPlus class="size-3.5" />
            {t("drift.registry.sources.addButton")}
          </button>
        </div>
        <div class="text-[0.7rem] text-ink-faint">{t(props.kind === "plugins" ? "drift.registry.sources.pluginsFormat" : "drift.registry.sources.mcpFormat")}</div>
      </div>
    </div>
  )
}
