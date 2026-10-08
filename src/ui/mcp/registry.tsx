import { IconArrowUp, IconArrowUpRight, IconCheck, IconKey, IconPlus, IconSearch, IconSliders } from "../icons"
import { loadRegistrySources, registrySources, sourcesOf } from "../../state/registry-sources"
import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js"
import { RegistrySourcesSheet } from "../registry-sources"
import { openExternal } from "../../shell"
import { useEngine } from "../../engine"
import { LogoTile } from "../logo-tile"
import { t } from "../../state/i18n"
import {
  preferredOption,
  registryInstallName,
  registryOptions,
  type InstallKind,
  type InstallOption,
  type RegistryServer,
} from "../../mcp-registry"
import {
  createRegistrySearch,
  forgetRegistryCatalog,
  loadCustomRegistry,
  registryScore,
} from "../../state/mcp-registry-search"

import type { McpServerConfig } from "../../engine/store"

type Filter = "all" | "remote" | "local"
type Entry = { server: RegistryServer; options: InstallOption[] }

const PAGE = 48
const KIND_LABEL: Record<InstallKind, string> = { remote: "", npm: "npm", pypi: "PyPI", docker: "Docker" }

/** Popular servers first, from GitHub's registry; a search also reaches the official one. */
export function McpRegistry(props: {
  installed: Set<string>
  disabled: boolean
  onInstall: (server: RegistryServer, config: McpServerConfig) => Promise<boolean>
}) {
  const [query, setQuery] = createSignal("")
  const [filter, setFilter] = createSignal<Filter>("all")
  const [popular, setPopular] = createSignal<Entry[]>([])
  const [more, setMore] = createSignal<Entry[]>([])
  const [loading, setLoading] = createSignal(true)
  const [searchingOfficial, setSearchingOfficial] = createSignal(false)
  const [error, setError] = createSignal("")
  const [shown, setShown] = createSignal(PAGE)
  const [selected, setSelected] = createSignal<Entry>()
  const [sourcesOpen, setSourcesOpen] = createSignal(false)
  const [own, setOwn] = createSignal<Entry[]>([])
  const [ownFailures, setOwnFailures] = createSignal<string[]>([])
  const engine = useEngine()
  const registry = createRegistrySearch()
  let disposed = false
  /** The user's own registries, read whole; one that fails is named and the rest still show. */
  const loadOwn = async () => {
    await loadRegistrySources({
      settings: () => engine.actions.engineSettings(),
      putSettings: (body) => engine.actions.putEngineSettings(body),
    }).catch(() => undefined)
    const sources = sourcesOf("mcp")
    const results = await Promise.allSettled(
      sources.map((source) => loadCustomRegistry(source, (id) => engine.actions.fetchRegistry(id))),
    )
    if (disposed) return
    setOwn(entries(results.flatMap((result) => (result.status === "fulfilled" ? result.value : []))))
    setOwnFailures(results.flatMap((result, index) => (result.status === "rejected" ? [sources[index]!.name] : [])))
  }
  const closeSources = () => {
    setSourcesOpen(false)
    void loadOwn()
  }
  const entries = (servers: RegistryServer[]) =>
    servers.map((server) => ({ server, options: registryOptions(server) })).filter((entry) => entry.options.length)
  const search = async () => {
    const asked = query()
    setLoading(true)
    setError("")
    setMore([])
    try {
      const result = await registry.search(asked)
      if (disposed || result.stale) return
      setPopular(entries(result.servers))
      setShown(PAGE)
      setLoading(false)
      void searchOfficial(asked, result.servers)
    } catch {
      if (disposed) return
      setError(t("drift.mcp.registryLoadFailed"))
      setLoading(false)
    }
  }
  /** The official registry answers slowly, so its matches join below GitHub's once they come; failing, they simply do not. */
  const searchOfficial = async (asked: string, shown: RegistryServer[]) => {
    setSearchingOfficial(asked.trim().length >= 2)
    const result = await registry.official(asked, shown).catch(() => ({ stale: false, servers: [] }))
    if (disposed || result.stale) return
    setMore(entries(result.servers))
    setSearchingOfficial(false)
  }
  onMount(() => {
    void search()
    void loadOwn()
  })
  let timer: number | undefined
  onCleanup(() => {
    disposed = true
    window.clearTimeout(timer)
    registry.dispose()
  })
  const schedule = (value: string) => {
    setQuery(value)
    window.clearTimeout(timer)
    timer = window.setTimeout(() => void search(), 200)
  }
  const matches = (entry: Entry) => {
    if (filter() === "remote") return entry.options.some((option) => option.kind === "remote")
    if (filter() === "local") return entry.options.some((option) => option.kind !== "remote")
    return true
  }
  const ownVisible = createMemo(() => {
    const asked = query().trim()
    return own()
      .filter(matches)
      .filter((entry) => !asked || registryScore(entry.server, asked) > 0)
  })
  const visible = createMemo(() => popular().filter(matches))
  const extra = createMemo(() => more().filter(matches))
  const installed = (server: RegistryServer) => props.installed.has(registryInstallName(server))

  return (
    <Show when={!sourcesOpen()} fallback={<RegistrySourcesSheet kind="mcp" onBack={closeSources} />}>
      <Show
        when={selected()}
        fallback={
          <div class="space-y-3">
            <div class="flex flex-wrap items-center gap-2">
              <label class="flex h-9 min-w-48 flex-1 items-center gap-2 rounded-md border border-edge bg-raised/45 px-2.5 focus-within:border-accent">
                <IconSearch class="size-3.5 shrink-0 text-ink-faint" />
                <input
                  aria-label={t("drift.mcp.registrySearch")}
                  placeholder={t("drift.mcp.registrySearch")}
                  class="min-w-0 flex-1 bg-transparent text-sm text-ink outline-none placeholder:text-ink-faint"
                  value={query()}
                  onInput={(event) => schedule(event.currentTarget.value)}
                />
              </label>
              <div
                class="flex rounded-lg border border-edge bg-surface p-0.5"
                role="group"
                aria-label={t("drift.mcp.registry.filter")}
              >
                <For each={["all", "remote", "local"] as Filter[]}>
                  {(value) => (
                    <button
                      type="button"
                      aria-pressed={filter() === value}
                      class="rounded-md px-2.5 py-1 text-xs"
                      classList={{
                        "bg-raised text-ink": filter() === value,
                        "text-ink-faint hover:text-ink": filter() !== value,
                      }}
                      onClick={() => setFilter(value)}
                    >
                      {t(`drift.mcp.registry.filter.${value}`)}
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
                <Show when={sourcesOf("mcp").length}>{(count) => <span>{count()}</span>}</Show>
              </button>
            </div>
            <div class="text-[0.7rem] text-ink-faint">{t("drift.mcp.registrySource")}</div>
            <For each={ownFailures()}>
              {(name) => (
                <div role="alert" class="rounded-md border border-warn/35 bg-warn/10 px-3 py-2 text-xs text-warn">
                  {t("drift.registry.sources.failed", { name, error: t("drift.plugins.registryLoadFailed") })}
                </div>
              )}
            </For>
            <Show when={ownVisible().length}>
              <div class="text-xs font-medium text-ink-muted">{t("drift.registry.sources.yours")}</div>
              <div class="grid gap-2 sm:grid-cols-2">
                <For each={ownVisible()}>
                  {(entry) => (
                    <RegistryCard entry={entry} installed={installed(entry.server)} onOpen={() => setSelected(entry)} />
                  )}
                </For>
              </div>
              <Show when={registrySources().length}>
                <div class="pt-1 text-xs font-medium text-ink-muted">{t("drift.mcp.registry")}</div>
              </Show>
            </Show>
            <Show when={error()}>
              <div
                role="alert"
                class="flex items-center gap-3 rounded-md border border-danger/35 bg-danger/10 px-3 py-2 text-xs text-danger"
              >
                {error()}
                <button
                  class="rounded border border-current px-2 py-0.5"
                  onClick={() => {
                    forgetRegistryCatalog()
                    void search()
                  }}
                >
                  {t("drift.mcp.retry")}
                </button>
              </div>
            </Show>
            <Show when={loading() && !popular().length}>
              <div class="grid gap-2 sm:grid-cols-2" aria-busy="true">
                <For each={Array.from({ length: 6 })}>
                  {() => <div class="h-[6.5rem] animate-pulse rounded-lg border border-edge bg-raised/30" />}
                </For>
              </div>
            </Show>
            <div class="grid gap-2 sm:grid-cols-2" aria-busy={loading()}>
              <For each={visible().slice(0, shown())}>
                {(entry) => (
                  <RegistryCard entry={entry} installed={installed(entry.server)} onOpen={() => setSelected(entry)} />
                )}
              </For>
            </div>
            <Show when={visible().length > shown()}>
              <button
                class="w-full rounded-md border border-edge py-2 text-xs text-ink-muted hover:text-ink"
                onClick={() => setShown(shown() + PAGE)}
              >
                {t("drift.mcp.registry.more", { count: visible().length - shown() })}
              </button>
            </Show>
            <Show when={searchingOfficial()}>
              <div role="status" class="flex items-center gap-2 pt-1 text-xs text-ink-faint">
                <span
                  aria-hidden="true"
                  class="size-3 rounded-full border-2 border-ink-faint/30 border-t-ink-muted motion-safe:animate-spin"
                />
                {t("drift.mcp.registry.searchingOfficial")}
              </div>
            </Show>
            <Show when={extra().length}>
              <div class="pt-2 text-xs font-medium text-ink-muted">{t("drift.mcp.registry.official")}</div>
              <div class="grid gap-2 sm:grid-cols-2">
                <For each={extra()}>
                  {(entry) => (
                    <RegistryCard entry={entry} installed={installed(entry.server)} onOpen={() => setSelected(entry)} />
                  )}
                </For>
              </div>
            </Show>
            <Show when={!loading() && !searchingOfficial() && !error() && !visible().length && !extra().length}>
              <div class="px-3 py-6 text-center text-sm text-ink-faint">{t("drift.mcp.registry.empty")}</div>
            </Show>
          </div>
        }
      >
        {(entry) => (
          <InstallSheet
            entry={entry()}
            installed={installed(entry().server)}
            disabled={props.disabled}
            onBack={() => setSelected()}
            onInstall={async (config) => {
              if (await props.onInstall(entry().server, config)) setSelected()
            }}
          />
        )}
      </Show>
    </Show>
  )
}

function RegistryCard(props: { entry: Entry; installed: boolean; onOpen: () => void }) {
  const server = () => props.entry.server
  return (
    <button
      type="button"
      class="group flex min-w-0 flex-col gap-2 rounded-lg border border-edge bg-surface p-3 text-left transition-colors hover:border-edge-strong hover:bg-raised/40 focus-visible:border-accent focus-visible:outline-none"
      onClick={props.onOpen}
    >
      <div class="flex min-w-0 items-start gap-2.5">
        <LogoTile image={server().listing?.image} title={title(server())} />
        <div class="min-w-0 flex-1">
          <div class="flex items-center gap-1.5">
            <span class="truncate text-sm font-medium text-ink">{title(server())}</span>
            <Show when={props.installed}>
              <IconCheck class="size-3.5 shrink-0 text-ok" aria-label={t("drift.mcp.installedLabel")} />
            </Show>
          </div>
          <Byline server={server()} />
        </div>
      </div>
      <div class="line-clamp-2 text-xs leading-relaxed text-ink-muted">{server().description}</div>
      <div class="mt-auto flex flex-wrap gap-1">
        <For each={badges(props.entry.options)}>{(badge) => <Badge tone={badge.tone}>{badge.text}</Badge>}</For>
      </div>
    </button>
  )
}

function InstallSheet(props: {
  entry: Entry
  installed: boolean
  disabled: boolean
  onBack: () => void
  onInstall: (config: McpServerConfig) => Promise<void>
}) {
  const server = () => props.entry.server
  const [optionId, setOptionId] = createSignal(preferredOption(props.entry.options)?.id ?? "")
  const [values, setValues] = createSignal<Record<string, string>>({})
  const [busy, setBusy] = createSignal(false)
  const option = () => props.entry.options.find((item) => item.id === optionId()) ?? props.entry.options[0]
  const config = createMemo(() => option()?.build(values()) ?? null)
  const listing = () => server().listing
  return (
    <div class="space-y-4">
      <button class="flex items-center gap-1.5 text-xs text-ink-faint hover:text-ink" onClick={props.onBack}>
        <IconArrowUp class="size-3.5 -rotate-90" />
        {t("drift.mcp.registry.back")}
      </button>
      <div class="flex items-start gap-3">
        <LogoTile image={server().listing?.image} title={title(server())} large />
        <div class="min-w-0 flex-1">
          <div class="text-base font-semibold text-ink">{title(server())}</div>
          <Byline server={server()} />
          <div class="mt-2 text-sm text-ink-muted">{server().description}</div>
          <div class="mt-2 flex flex-wrap items-center gap-3 text-xs">
            <Show when={listing()?.repository}>
              {(url) => <ExternalLink url={url()}>{t("drift.mcp.registry.repository")}</ExternalLink>}
            </Show>
            <Show when={listing()?.website}>
              {(url) => <ExternalLink url={url()}>{t("drift.mcp.registry.website")}</ExternalLink>}
            </Show>
            <span class="text-ink-faint">{server().name}</span>
          </div>
          <Show when={listing()?.topics?.length}>
            <div class="mt-2 flex flex-wrap gap-1">
              <For each={listing()!.topics!.slice(0, 8)}>{(topic) => <Badge>{topic}</Badge>}</For>
            </div>
          </Show>
        </div>
      </div>
      <Show when={props.entry.options.length > 1}>
        <div class="space-y-1.5">
          <div class="text-xs font-medium text-ink-muted">{t("drift.mcp.registry.runAs")}</div>
          <div class="flex flex-wrap gap-1.5">
            <For each={props.entry.options}>
              {(item) => (
                <button
                  type="button"
                  aria-pressed={item.id === optionId()}
                  class="rounded-md border px-2.5 py-1.5 text-left text-xs"
                  classList={{
                    "border-accent bg-accent/10 text-ink": item.id === optionId(),
                    "border-edge text-ink-muted hover:text-ink": item.id !== optionId(),
                  }}
                  onClick={() => setOptionId(item.id)}
                >
                  {optionLabel(item)}
                </button>
              )}
            </For>
          </div>
        </div>
      </Show>
      <Show when={option()}>
        {(current) => (
          <div class="space-y-3 rounded-lg border border-edge bg-surface p-3">
            <div class="text-xs text-ink-muted">{optionNote(current())}</div>
            <For each={current().fields}>
              {(field) => (
                <label class="block space-y-1">
                  <div class="flex items-baseline gap-2 text-xs">
                    <span class="font-medium text-ink">{field.label}</span>
                    <span class="text-ink-faint">
                      {t(field.required ? "drift.mcp.registry.required" : "drift.mcp.registry.optional")}
                    </span>
                    <Show when={field.secret}>
                      <IconKey class="size-3 text-ink-faint" aria-label={t("drift.mcp.registry.secret")} />
                    </Show>
                  </div>
                  <input
                    type={field.secret ? "password" : "text"}
                    autocomplete="off"
                    spellcheck={false}
                    class="h-8 w-full rounded-md border border-edge bg-raised/45 px-2.5 font-mono text-xs text-ink outline-none focus:border-accent"
                    placeholder={field.default ?? ""}
                    value={values()[field.key] ?? ""}
                    onInput={(event) => setValues({ ...values(), [field.key]: event.currentTarget.value })}
                  />
                  <Show when={field.description}>
                    {(text) => <div class="text-[0.7rem] text-ink-faint">{text()}</div>}
                  </Show>
                </label>
              )}
            </For>
            <Show when={current().fields.some((field) => field.secret)}>
              <div class="text-[0.7rem] text-ink-faint">{t("drift.mcp.registry.secretNote")}</div>
            </Show>
          </div>
        )}
      </Show>
      <div class="flex items-center justify-end gap-2">
        <button class="rounded-md px-3 py-1.5 text-xs text-ink-muted hover:text-ink" onClick={props.onBack}>
          {t("common.cancel")}
        </button>
        <button
          class="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-40"
          disabled={props.disabled || props.installed || busy() || !config()}
          onClick={async () => {
            const ready = config()
            if (!ready) return
            setBusy(true)
            try {
              await props.onInstall(ready)
            } finally {
              setBusy(false)
            }
          }}
        >
          {props.installed ? <IconCheck class="size-3.5" /> : <IconPlus class="size-3.5" />}
          {t(
            props.installed
              ? "drift.mcp.installedLabel"
              : busy()
                ? "drift.mcp.registry.installing"
                : "drift.mcp.install",
          )}
        </button>
      </div>
    </div>
  )
}

function title(server: RegistryServer) {
  return server.title ?? server.name.split("/").at(-1) ?? server.name
}

function Byline(props: { server: RegistryServer }) {
  const listing = () => props.server.listing
  return (
    <div class="flex items-center gap-1.5 truncate text-[0.7rem] text-ink-faint">
      <Show when={listing()?.sourceName}>
        {(name) => <span class="shrink-0 rounded bg-warn/12 px-1 text-warn">{name()}</span>}
      </Show>
      <Show when={listing()?.publisher}>{(publisher) => <span class="truncate">{publisher()}</span>}</Show>
      <Show when={listing()?.stars}>
        {(stars) => (
          <span class="shrink-0" title={t("drift.mcp.registry.stars", { count: stars() })}>
            ★ {compact(stars())}
          </span>
        )}
      </Show>
    </div>
  )
}

function Badge(props: { tone?: "accent" | "warn"; children: JSX.Element }) {
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

function ExternalLink(props: { url: string; children: JSX.Element }) {
  return (
    <button class="flex items-center gap-0.5 text-accent hover:underline" onClick={() => openExternal(props.url)}>
      {props.children}
      <IconArrowUpRight class="size-3" />
    </button>
  )
}

/** What a card says about how it runs: remote or which package, and whether a key has to be typed first. */
export function registryBadges(options: InstallOption[]) {
  return badges(options).map((badge) => badge.text)
}

function badges(options: InstallOption[]) {
  const result: { text: string; tone?: "accent" | "warn" }[] = []
  if (options.some((option) => option.kind === "remote"))
    result.push({ text: t("drift.mcp.registry.filter.remote"), tone: "accent" })
  for (const kind of ["npm", "pypi", "docker"] as InstallKind[]) {
    if (options.some((option) => option.kind === kind)) result.push({ text: KIND_LABEL[kind] })
  }
  if (preferredOption(options)?.fields.some((field) => field.required))
    result.push({ text: t("drift.mcp.registry.needsKey"), tone: "warn" })
  return result
}

function optionLabel(option: InstallOption) {
  if (option.kind === "remote")
    return `${t("drift.mcp.registry.filter.remote")} · ${option.detail === "sse" ? t("drift.mcp.transport.sse") : t("drift.mcp.transport.streamable_http")}`
  return `${KIND_LABEL[option.kind]} · ${option.detail}`
}

function optionNote(option: InstallOption) {
  if (option.kind === "remote") return t("drift.mcp.registry.note.remote")
  if (option.kind === "docker") return t("drift.mcp.registry.note.docker")
  if (option.detail === "latest")
    return t("drift.mcp.registry.note.latest", { runtime: option.kind === "npm" ? "npx" : "uvx" })
  return t("drift.mcp.registry.note.local", { runtime: option.kind === "npm" ? "npx" : "uvx" })
}

function compact(count: number) {
  return new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 }).format(count)
}
