import { loadRegistrySources, registrySources, sourcesOf } from "../state/registry-sources"
import { createMemo, createSignal, For, onMount, Show, type JSX } from "solid-js"
import { RegistrySourcesSheet } from "./registry-sources"
import { openExternal } from "../shell"
import { LogoTile } from "./logo-tile"
import { useEngine } from "../engine"
import { Toggle } from "./controls"
import { t } from "../state/i18n"
import {
  buildConfig,
  fieldText,
  installedPath,
  isSkillEntry,
  loadRegistries,
  matchesRegistryQuery,
  registryCategories,
  type ConfigField,
  type RegistryFailure,
  type RegistryPlugin,
} from "../state/plugin-registry"
import {
  IconArrowUp,
  IconArrowUpRight,
  IconCheck,
  IconPlus,
  IconSearch,
  IconSliders,
  IconSquarePen,
  IconTrash,
} from "./icons"

import type { PluginInfo } from "../engine/native/client"

type View = "installed" | "registry"
const configFile = "~/.config/drift/drift.json"

/** Plugins as MCP servers are shown: what is installed, and a registry to install from. */
export function PluginsSection() {
  const engine = useEngine()
  const [view, setView] = createSignal<View>("installed")
  const [plugins, setPlugins] = createSignal<PluginInfo[]>([])
  const [loading, setLoading] = createSignal(false)
  const [busy, setBusy] = createSignal("")
  const [failure, setFailure] = createSignal("")
  const [message, setMessage] = createSignal("")
  const [confirmRemove, setConfirmRemove] = createSignal("")
  const [editing, setEditing] = createSignal<PluginInfo>()
  const locked = () => engine.state.connection !== "online" || loading() || !!busy()

  const run = async (action: () => Promise<PluginInfo[]>, success?: string) => {
    setLoading(true)
    setFailure("")
    setMessage("")
    try {
      setPlugins(await action())
      if (success) setMessage(success)
      return true
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error))
      return false
    } finally {
      setLoading(false)
    }
  }
  onMount(() => void run(() => engine.actions.plugins()))

  const install = async (plugin: RegistryPlugin, config: Record<string, unknown>) => {
    setBusy(plugin.id)
    const done = await run(
      () =>
        engine.actions.installPlugin({
          id: plugin.id,
          url: plugin.download,
          sha256: plugin.sha256,
          config,
          registry: plugin.sourceId,
        }),
      t("drift.plugins.installed.one", { name: plugin.name }),
    )
    setBusy("")
    if (done) setView("installed")
    return done
  }
  const remove = async (plugin: PluginInfo) => {
    if (confirmRemove() !== plugin.path) return setConfirmRemove(plugin.path)
    setBusy(plugin.path)
    await run(() => engine.actions.removePlugin(plugin.path), t("drift.plugins.removed", { name: plugin.name }))
    setBusy("")
    setConfirmRemove("")
  }
  const installedPaths = createMemo(() => new Set(plugins().map((plugin) => plugin.path)))
  // Installed rows show the registry's picture and edit with its fields, for the plugin at their path.
  const [known, setKnown] = createSignal<Record<string, RegistryPlugin>>({})
  onMount(() => {
    void loadRegistrySources({
      settings: () => engine.actions.engineSettings(),
      putSettings: (body) => engine.actions.putEngineSettings(body),
    })
      .catch(() => undefined)
      .then(() => loadRegistries(sourcesOf("plugins"), false, (id) => engine.actions.fetchRegistry(id)))
      .then((loaded) =>
        setKnown(Object.fromEntries(loaded.plugins.map((plugin) => [installedPath(plugin.id), plugin]))),
      )
      .catch(() => undefined)
  })
  const save = async (plugin: PluginInfo, config: unknown) => {
    setBusy(plugin.path)
    const done = await run(
      () => engine.actions.configurePlugin(plugin.path, config),
      t("drift.plugins.saved", { name: plugin.name }),
    )
    setBusy("")
    if (done) setEditing(undefined)
  }

  return (
    <div class="space-y-3">
      <div class="flex items-center justify-between gap-3">
        <div class="flex rounded-lg border border-edge bg-surface p-0.5">
          <Tab active={view() === "installed"} onClick={() => setView("installed")}>
            {t("drift.plugins.tab.installed")}
          </Tab>
          <Tab active={view() === "registry"} onClick={() => setView("registry")}>
            {t("drift.plugins.tab.registry")}
          </Tab>
        </div>
        <Show when={view() === "installed"}>
          <button
            class="rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
            disabled={locked()}
            onClick={() => void run(() => engine.actions.reloadPlugins())}
          >
            {t("drift.plugins.reload")}
          </button>
        </Show>
      </div>
      <Show when={failure() || message()}>
        <div
          role={failure() ? "alert" : "status"}
          class="rounded-md border px-3 py-2 text-xs"
          classList={{
            "border-danger/35 bg-danger/10 text-danger": !!failure(),
            "border-ok/35 bg-ok/10 text-ok": !failure(),
          }}
        >
          {failure() || message()}
        </div>
      </Show>
      <Show when={view() === "installed" && editing()}>
        {(plugin) => (
          <EditSheet
            plugin={plugin()}
            registry={known()[plugin().path]}
            busy={busy() === plugin().path}
            onBack={() => setEditing(undefined)}
            onSave={(config) => save(plugin(), config)}
          />
        )}
      </Show>
      <Show when={view() === "installed" && !editing()}>
        <div class="border-y border-edge/80" aria-busy={loading()}>
          <For each={plugins()}>
            {(plugin) => (
              <PluginRow
                plugin={plugin}
                image={known()[plugin.path]?.image}
                disabled={locked()}
                confirming={confirmRemove() === plugin.path}
                onEnabled={(enabled) => void run(() => engine.actions.setPluginEnabled(plugin.path, enabled))}
                onEdit={() => setEditing(plugin)}
                onRemove={() => void remove(plugin)}
              />
            )}
          </For>
          <Show when={!loading() && !plugins().length}>
            <div class="px-3 py-5 text-sm text-ink-faint">{t("drift.plugins.empty", { path: configFile })}</div>
          </Show>
        </div>
      </Show>
      <Show when={view() === "registry"}>
        <PluginRegistry installed={installedPaths()} disabled={locked()} busy={busy()} onInstall={install} />
      </Show>
    </div>
  )
}

function PluginRow(props: {
  plugin: PluginInfo
  image?: string
  disabled: boolean
  confirming: boolean
  onEnabled: (enabled: boolean) => void
  onEdit: () => void
  onRemove: () => void
}) {
  const status = () =>
    !props.plugin.enabled ? t("drift.plugins.off") : (props.plugin.error ?? t("drift.plugins.loaded"))
  return (
    <div class="flex items-center gap-3 border-b border-edge/70 px-3 py-2.5 last:border-b-0 hover:bg-raised/40">
      <LogoTile image={props.image} title={props.plugin.name} />
      <div class="min-w-0 flex-1">
        <div class="truncate text-sm font-medium text-ink">{props.plugin.name}</div>
        <div class="mt-0.5 flex min-w-0 flex-wrap items-center gap-x-2 text-xs">
          <span
            classList={{
              "text-ok": props.plugin.enabled && !props.plugin.error,
              "text-danger": props.plugin.enabled && !!props.plugin.error,
              "text-ink-faint": !props.plugin.enabled,
            }}
            title={props.plugin.error ?? undefined}
          >
            {status()}
          </span>
          <Show when={props.plugin.capabilities?.length}>
            <span class="text-ink-muted">
              {props.plugin.capabilities?.map((name) => t(`drift.plugins.capability.${name}`)).join(" · ")}
            </span>
          </Show>
          <span class="truncate font-mono text-ink-faint">{props.plugin.path}</span>
        </div>
      </div>
      <div class="flex shrink-0 items-center gap-1.5">
        <button
          type="button"
          disabled={props.disabled}
          title={props.confirming ? t("drift.plugins.confirmRemove") : t("drift.plugins.remove")}
          aria-label={props.confirming ? t("drift.plugins.confirmRemove") : t("drift.plugins.remove")}
          class="flex items-center gap-1 rounded-md border border-danger/40 px-2 py-1 text-xs text-danger hover:bg-danger/10 disabled:opacity-40"
          onClick={() => props.onRemove()}
        >
          {props.confirming ? t("drift.plugins.confirmRemove") : <IconTrash class="size-3.5" />}
        </button>
        <button
          type="button"
          disabled={props.disabled}
          title={t("common.edit")}
          aria-label={t("common.edit")}
          class="flex items-center rounded-md border border-edge px-2 py-1 text-xs text-ink-muted hover:text-ink disabled:opacity-40"
          onClick={() => props.onEdit()}
        >
          <IconSquarePen class="size-3.5" />
        </button>
        <Toggle
          label={props.plugin.name}
          checked={props.plugin.enabled}
          disabled={props.disabled}
          onChange={() => props.onEnabled(!props.plugin.enabled)}
        />
      </div>
    </div>
  )
}

function PluginRegistry(props: {
  installed: Set<string>
  disabled: boolean
  busy: string
  onInstall: (plugin: RegistryPlugin, config: Record<string, unknown>) => Promise<boolean>
}) {
  const engine = useEngine()
  const [query, setQuery] = createSignal("")
  const [category, setCategory] = createSignal("all")
  const [plugins, setPlugins] = createSignal<RegistryPlugin[]>([])
  const [failures, setFailures] = createSignal<RegistryFailure[]>([])
  const [loading, setLoading] = createSignal(true)
  const [error, setError] = createSignal("")
  const [selected, setSelected] = createSignal<RegistryPlugin>()
  const [sourcesOpen, setSourcesOpen] = createSignal(false)
  const load = async (fresh = false) => {
    setLoading(true)
    setError("")
    try {
      await loadRegistrySources({
        settings: () => engine.actions.engineSettings(),
        putSettings: (body) => engine.actions.putEngineSettings(body),
      }).catch(() => undefined)
      const loaded = await loadRegistries(sourcesOf("plugins"), fresh, (id) => engine.actions.fetchRegistry(id))
      setPlugins(loaded.plugins.filter((plugin) => !isSkillEntry(plugin)))
      setFailures(loaded.failures)
      if (!loaded.plugins.length && loaded.failures.length) setError(t("drift.plugins.registryLoadFailed"))
    } catch {
      setError(t("drift.plugins.registryLoadFailed"))
    } finally {
      setLoading(false)
    }
  }
  onMount(() => void load())
  // Sources changed in the sheet: the list reflects them when it comes back.
  const closeSources = () => {
    setSourcesOpen(false)
    void load(true)
  }
  const visible = createMemo(() =>
    plugins().filter(
      (plugin) => (category() === "all" || plugin.category === category()) && matchesRegistryQuery(plugin, query()),
    ),
  )
  const installed = (plugin: RegistryPlugin) => props.installed.has(installedPath(plugin.id))

  async function installSelected(plugin: RegistryPlugin, config: Record<string, unknown>) {
    if (await props.onInstall(plugin, config)) setSelected()
  }

  return (
    <Show when={!sourcesOpen()} fallback={<RegistrySourcesSheet kind="plugins" onBack={closeSources} />}>
      <Show
        when={selected()}
        fallback={
          <div class="space-y-3">
            <div class="flex flex-wrap items-center gap-2">
              <label class="flex h-9 min-w-48 flex-1 items-center gap-2 rounded-md border border-edge bg-raised/45 px-2.5 focus-within:border-accent">
                <IconSearch class="size-3.5 shrink-0 text-ink-faint" />
                <input
                  aria-label={t("drift.plugins.registrySearch")}
                  placeholder={t("drift.plugins.registrySearch")}
                  class="min-w-0 flex-1 bg-transparent text-sm text-ink outline-none placeholder:text-ink-faint"
                  value={query()}
                  onInput={(event) => setQuery(event.currentTarget.value)}
                />
              </label>
              <div
                class="flex rounded-lg border border-edge bg-surface p-0.5"
                role="group"
                aria-label={t("drift.plugins.category")}
              >
                <For each={["all", ...registryCategories]}>
                  {(value) => (
                    <button
                      type="button"
                      aria-pressed={category() === value}
                      class="rounded-md px-2.5 py-1 text-xs"
                      classList={{
                        "bg-raised text-ink": category() === value,
                        "text-ink-faint hover:text-ink": category() !== value,
                      }}
                      onClick={() => setCategory(value)}
                    >
                      {t(`drift.plugins.category.${value}`)}
                    </button>
                  )}
                </For>
              </div>
              <button
                type="button"
                title={t("drift.registry.sources")}
                aria-label={t("drift.registry.sources")}
                class="flex h-9 items-center gap-1.5 rounded-md border border-edge px-2.5 text-xs text-ink-muted hover:border-edge-strong hover:text-ink"
                onClick={() => setSourcesOpen(true)}
              >
                <IconSliders class="size-3.5" />
                <Show when={sourcesOf("plugins").length}>{(count) => <span>{count()}</span>}</Show>
              </button>
            </div>
            <div class="text-[0.7rem] text-ink-faint">
              {t(
                registrySources().some((source) => source.kind === "plugins")
                  ? "drift.plugins.registrySourceWithOwn"
                  : "drift.plugins.registrySource",
              )}
            </div>
            <For each={failures()}>
              {(failure) => (
                <div role="alert" class="rounded-md border border-warn/35 bg-warn/10 px-3 py-2 text-xs text-warn">
                  {t("drift.registry.sources.failed", { name: failure.name, error: failure.error })}
                </div>
              )}
            </For>
            <Show when={error()}>
              <div
                role="alert"
                class="flex items-center gap-3 rounded-md border border-danger/35 bg-danger/10 px-3 py-2 text-xs text-danger"
              >
                {error()}
                <button class="rounded border border-current px-2 py-0.5" onClick={() => void load(true)}>
                  {t("drift.mcp.retry")}
                </button>
              </div>
            </Show>
            <Show when={loading() && !plugins().length}>
              <div class="grid gap-2 sm:grid-cols-2" aria-busy="true">
                <For each={Array.from({ length: 6 })}>
                  {() => <div class="h-[6.5rem] animate-pulse rounded-lg border border-edge bg-raised/30" />}
                </For>
              </div>
            </Show>
            <div class="grid gap-2 sm:grid-cols-2" aria-busy={loading()}>
              <For each={visible()}>
                {(plugin) => (
                  <RegistryCard plugin={plugin} installed={installed(plugin)} onOpen={() => setSelected(plugin)} />
                )}
              </For>
            </div>
            <Show when={!loading() && !error() && !visible().length}>
              <div class="px-3 py-6 text-center text-sm text-ink-faint">{t("drift.plugins.registryEmpty")}</div>
            </Show>
          </div>
        }
      >
        {(plugin) => (
          <InstallSheet
            plugin={plugin()}
            installed={installed(plugin())}
            disabled={props.disabled}
            busy={props.busy === plugin().id}
            onBack={() => setSelected()}
            onInstall={(config) => installSelected(plugin(), config)}
          />
        )}
      </Show>
    </Show>
  )
}

function RegistryCard(props: { plugin: RegistryPlugin; installed: boolean; onOpen: () => void }) {
  return (
    <button
      type="button"
      class="group flex min-w-0 flex-col gap-2 rounded-lg border border-edge bg-surface p-3 text-left transition-colors hover:border-edge-strong hover:bg-raised/40 focus-visible:border-accent focus-visible:outline-none"
      onClick={() => props.onOpen()}
    >
      <div class="flex min-w-0 items-start gap-2.5">
        <LogoTile image={props.plugin.image} title={props.plugin.name} />
        <div class="min-w-0 flex-1">
          <div class="flex items-center gap-1.5">
            <span class="truncate text-sm font-medium text-ink">{props.plugin.name}</span>
            <Show when={props.installed}>
              <IconCheck class="size-3.5 shrink-0 text-ok" aria-label={t("drift.plugins.installedLabel")} />
            </Show>
          </div>
          <div class="truncate text-[0.7rem] text-ink-faint">
            {props.plugin.author} · v{props.plugin.version}
          </div>
        </div>
      </div>
      <div class="line-clamp-2 text-xs leading-relaxed text-ink-muted">{props.plugin.description}</div>
      <div class="mt-auto flex flex-wrap gap-1">
        <Show when={props.plugin.sourceName}>{(name) => <Badge tone="warn">{name()}</Badge>}</Show>
        <Badge tone="accent">{t(`drift.plugins.category.${props.plugin.category}`)}</Badge>
        <For each={props.plugin.hooks}>{(hook) => <Badge>{hook}</Badge>}</For>
      </div>
    </button>
  )
}

/** The fields of a plugin's config as inputs; `values` are what is stored now, the defaults stand in for the rest. */
function ConfigFields(props: {
  fields: ConfigField[]
  typed: Record<string, string>
  values: Record<string, unknown>
  onTyped: (typed: Record<string, string>) => void
}) {
  const value = (field: ConfigField) =>
    props.typed[field.key] ?? fieldText(field, field.key in props.values ? props.values[field.key] : field.default)
  const set = (field: ConfigField, text: string) => props.onTyped({ ...props.typed, [field.key]: text })
  return (
    <div class="space-y-3 rounded-lg border border-edge bg-surface p-3">
      <div class="text-xs text-ink-muted">{t("drift.plugins.configNote")}</div>
      <For each={props.fields}>
        {(field) => (
          <Show
            when={field.type !== "boolean"}
            fallback={
              <div class="flex items-center justify-between gap-3">
                <div class="min-w-0">
                  <div class="text-xs font-medium text-ink">{field.label}</div>
                  <Show when={field.description}>
                    {(text) => <div class="text-[0.7rem] text-ink-faint">{text()}</div>}
                  </Show>
                </div>
                <Toggle
                  label={field.label}
                  checked={value(field) === "true"}
                  onChange={() => set(field, value(field) === "true" ? "false" : "true")}
                />
              </div>
            }
          >
            <label class="block space-y-1">
              <div class="flex items-baseline gap-2 text-xs">
                <span class="font-medium text-ink">{field.label}</span>
                <span class="text-ink-faint">{t(`drift.plugins.fieldType.${field.type}`)}</span>
              </div>
              <Show
                when={field.type === "json"}
                fallback={
                  <input
                    type="text"
                    autocomplete="off"
                    spellcheck={false}
                    class="h-8 w-full rounded-md border border-edge bg-raised/45 px-2.5 font-mono text-xs text-ink outline-none focus:border-accent"
                    value={value(field)}
                    onInput={(event) => set(field, event.currentTarget.value)}
                  />
                }
              >
                <textarea
                  spellcheck={false}
                  class="h-32 w-full resize-y rounded-md border border-edge bg-raised/45 p-2.5 font-mono text-xs text-ink outline-none focus:border-accent"
                  value={value(field)}
                  onInput={(event) => set(field, event.currentTarget.value)}
                />
              </Show>
              <Show when={field.description}>{(text) => <div class="text-[0.7rem] text-ink-faint">{text()}</div>}</Show>
            </label>
          </Show>
        )}
      </For>
    </div>
  )
}

/** An installed plugin's settings: the registry's fields when a registry describes it, the raw object otherwise. */
function EditSheet(props: {
  plugin: PluginInfo
  registry?: RegistryPlugin
  busy: boolean
  onBack: () => void
  onSave: (config: unknown) => Promise<void>
}) {
  const stored = () =>
    props.plugin.config && typeof props.plugin.config === "object"
      ? (props.plugin.config as Record<string, unknown>)
      : {}
  const [typed, setTyped] = createSignal<Record<string, string>>({})
  const [raw, setRaw] = createSignal(JSON.stringify(stored(), null, 2))
  const rawValid = () => {
    try {
      const parsed: unknown = JSON.parse(raw())
      return !!parsed && typeof parsed === "object" && !Array.isArray(parsed)
    } catch {
      return false
    }
  }
  const config = () =>
    props.registry
      ? buildConfig(props.registry.config, {
          ...Object.fromEntries(
            props.registry.config.map((field) => [
              field.key,
              fieldText(field, field.key in stored() ? stored()[field.key] : field.default),
            ]),
          ),
          ...typed(),
        })
      : JSON.parse(raw())
  return (
    <div class="space-y-4">
      <button class="flex items-center gap-1.5 text-xs text-ink-faint hover:text-ink" onClick={() => props.onBack()}>
        <IconArrowUp class="size-3.5 -rotate-90" />
        {t("drift.mcp.registry.back")}
      </button>
      <div class="flex items-start gap-3">
        <LogoTile image={props.registry?.image} title={props.plugin.name} large />
        <div class="min-w-0 flex-1">
          <div class="text-base font-semibold text-ink">{props.plugin.name}</div>
          <div class="font-mono text-[0.7rem] text-ink-faint">{props.plugin.path}</div>
          <Show when={props.registry?.description}>
            {(text) => <div class="mt-2 text-sm text-ink-muted">{text()}</div>}
          </Show>
        </div>
      </div>
      <Show
        when={props.registry?.config.length}
        fallback={
          <div class="space-y-2 rounded-lg border border-edge bg-surface p-3">
            <div class="text-xs text-ink-muted">{t("drift.plugins.configRaw")}</div>
            <textarea
              spellcheck={false}
              aria-label={t("drift.plugins.configRaw")}
              class="h-48 w-full resize-y rounded-md border border-edge bg-raised/45 p-2.5 font-mono text-xs text-ink outline-none focus:border-accent"
              classList={{ "border-danger/60": !rawValid() }}
              value={raw()}
              onInput={(event) => setRaw(event.currentTarget.value)}
            />
          </div>
        }
      >
        <ConfigFields fields={props.registry!.config} typed={typed()} onTyped={setTyped} values={stored()} />
      </Show>
      <div class="flex items-center justify-end gap-2">
        <button class="rounded-md px-3 py-1.5 text-xs text-ink-muted hover:text-ink" onClick={() => props.onBack()}>
          {t("common.cancel")}
        </button>
        <button
          class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-40"
          disabled={props.busy || (!props.registry?.config.length && !rawValid())}
          onClick={() => void props.onSave(config())}
        >
          {t(props.busy ? "drift.plugins.saving" : "common.save")}
        </button>
      </div>
    </div>
  )
}

function InstallSheet(props: {
  plugin: RegistryPlugin
  installed: boolean
  disabled: boolean
  busy: boolean
  onBack: () => void
  onInstall: (config: Record<string, unknown>) => Promise<void>
}) {
  const [typed, setTyped] = createSignal<Record<string, string>>({})
  return (
    <div class="space-y-4">
      <button class="flex items-center gap-1.5 text-xs text-ink-faint hover:text-ink" onClick={() => props.onBack()}>
        <IconArrowUp class="size-3.5 -rotate-90" />
        {t("drift.mcp.registry.back")}
      </button>
      <div class="flex items-start gap-3">
        <LogoTile image={props.plugin.image} title={props.plugin.name} large />
        <div class="min-w-0 flex-1">
          <div class="text-base font-semibold text-ink">{props.plugin.name}</div>
          <div class="text-[0.7rem] text-ink-faint">
            {props.plugin.author} · v{props.plugin.version} · {formatSize(props.plugin.size)}
          </div>
          <div class="mt-2 text-sm text-ink-muted">{props.plugin.description}</div>
          <div class="mt-2 flex flex-wrap items-center gap-3 text-xs">
            <button
              class="flex items-center gap-0.5 text-accent hover:underline"
              onClick={() => openExternal(props.plugin.source)}
            >
              {t("drift.plugins.source")}
              <IconArrowUpRight class="size-3" />
            </button>
            <span class="font-mono text-ink-faint">{installedPath(props.plugin.id)}</span>
          </div>
          <div class="mt-2 flex flex-wrap gap-1">
            <Show when={props.plugin.sourceName}>{(name) => <Badge tone="warn">{name()}</Badge>}</Show>
            <Badge tone="accent">{t(`drift.plugins.category.${props.plugin.category}`)}</Badge>
            <For each={props.plugin.hooks}>{(hook) => <Badge>{hook}</Badge>}</For>
          </div>
        </div>
      </div>
      <Show when={props.plugin.config.length}>
        <ConfigFields fields={props.plugin.config} typed={typed()} onTyped={setTyped} values={{}} />
      </Show>

      <div class="flex items-center justify-end gap-2">
        <button class="rounded-md px-3 py-1.5 text-xs text-ink-muted hover:text-ink" onClick={() => props.onBack()}>
          {t("common.cancel")}
        </button>
        <button
          class="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-40"
          disabled={props.disabled || props.installed || props.busy}
          onClick={() => void props.onInstall(buildConfig(props.plugin.config, typed()))}
        >
          {props.installed ? <IconCheck class="size-3.5" /> : <IconPlus class="size-3.5" />}
          {t(pluginInstallLabel(props.installed, props.busy))}
        </button>
      </div>
    </div>
  )
}

export function Badge(props: { tone?: "accent" | "warn"; children: JSX.Element }) {
  return (
    <span
      class="rounded px-1.5 py-0.5 text-[0.65rem]"
      classList={{
        "bg-raised text-ink-muted": !props.tone,
        "bg-accent/12 text-accent": props.tone === "accent",
        "bg-warn/12 text-warn": props.tone === "warn",
      }}
    >
      {props.children}
    </span>
  )
}

export function Tab(props: { active: boolean; onClick: () => void; children: JSX.Element }) {
  return (
    <button
      type="button"
      aria-pressed={props.active}
      class="min-w-0 flex-1 rounded-md px-2.5 py-1.5 text-xs"
      classList={{ "bg-raised text-ink": props.active, "text-ink-faint hover:text-ink": !props.active }}
      onClick={() => props.onClick()}
    >
      {props.children}
    </button>
  )
}

function formatSize(bytes: number) {
  return bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : `${Math.round(bytes / 1024)} KB`
}

function pluginInstallLabel(installed: boolean, busy: boolean) {
  if (installed) return "drift.plugins.installedLabel"
  if (busy) return "drift.plugins.installing"

  return "drift.plugins.install"
}
