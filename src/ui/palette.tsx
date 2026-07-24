import { createEffect, createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js"
import { useEngine } from "../engine"
import { sessionsFor } from "../engine/store"
import { onKeybind } from "../state/keybinds"
import { selectSession } from "../state/selection"
import { setTheme, theme, themes } from "../state/theme"
import { archivedIds, selectWorkspace, workspaces } from "../state/workspaces"
import { openMcpServers } from "./mcp"
import { closeOnBackdropPointerDown } from "./modal"
import { openSettings } from "./settings"
import { t } from "../state/i18n"

type PaletteItem = { label: string; hint: string; run: () => void }

const [open, setOpen] = createSignal(false)

export function openPalette() {
  setOpen(true)
}

export function PaletteHost() {
  onMount(() => {
    onKeybind("palette", () => setOpen(!open()))
    onKeybind("newThread", () => selectSession(null))
  })
  return (
    <Show when={open()}>
      <Palette onClose={() => setOpen(false)} />
    </Show>
  )
}

function Palette(props: { onClose: () => void }) {
  const engine = useEngine()
  const [query, setQuery] = createSignal("")
  const [cursor, setCursor] = createSignal(0)

  createEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") props.onClose()
    }
    document.addEventListener("keydown", escape)
    onCleanup(() => document.removeEventListener("keydown", escape))
  })

  const items = createMemo<PaletteItem[]>(() => {
    const actions: PaletteItem[] = [
      { label: t("drift.thread.new"), hint: t("command.category.suggested"), run: () => selectSession(null) },
      { label: t("sidebar.settings"), hint: "action", run: openSettings },
      { label: t("dialog.mcp.title"), hint: t("command.category.suggested"), run: openMcpServers },
      {
        label: t("command.theme.cycle"),
        hint: t("command.category.suggested"),
        run: () => setTheme(themes[(themes.indexOf(theme()) + 1) % themes.length]),
      },
    ]
    const spaces = workspaces().map((workspace) => ({
      label: workspace.name,
      hint: t("command.category.workspace"),
      run: () => selectWorkspace(workspace.id),
    }))
    const threads = workspaces().flatMap((workspace) =>
      sessionsFor(engine.state, workspace.path)
        .filter((session) => !archivedIds().has(session.id))
        .map((session) => ({
          label: session.title || t("drift.thread.untitled"),
          hint: workspace.name,
          run: () => {
            selectWorkspace(workspace.id)
            selectSession(session.id)
          },
        })),
    )
    return [...actions, ...spaces, ...threads]
  })

  const filtered = createMemo(() => {
    const value = query().toLowerCase()
    return items()
      .filter((item) => item.label.toLowerCase().includes(value) || item.hint.toLowerCase().includes(value))
      .slice(0, 12)
  })

  const pick = (item: PaletteItem) => {
    props.onClose()
    item.run()
  }

  const onKey = (event: KeyboardEvent) => {
    if (event.key === "ArrowDown") setCursor(Math.min(cursor() + 1, filtered().length - 1))
    else if (event.key === "ArrowUp") setCursor(Math.max(cursor() - 1, 0))
    else if (event.key === "Enter" && filtered()[Math.min(cursor(), filtered().length - 1)])
      pick(filtered()[Math.min(cursor(), filtered().length - 1)])
    else return
    event.preventDefault()
  }

  return (
    <div
      class="fixed inset-0 z-40 flex justify-center bg-black/50 pt-[18vh]"
      onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose)}
    >
      <div
        class="fade-up flex h-fit max-h-[50vh] w-[34rem] flex-col overflow-hidden rounded-xl border border-edge bg-overlay shadow-2xl shadow-black/40"
        onClick={(event) => event.stopPropagation()}
      >
        <input
          autofocus
          class="border-b border-edge bg-transparent px-4 py-3 text-sm outline-none placeholder:text-ink-faint"
          placeholder={t("palette.search.placeholder")}
          value={query()}
          onInput={(event) => {
            setQuery(event.currentTarget.value)
            setCursor(0)
          }}
          onKeyDown={onKey}
        />
        <div class="min-h-0 flex-1 overflow-y-auto py-1">
          <For each={filtered()}>
            {(item, index) => (
              <button
                class="flex w-full items-center gap-3 px-4 py-1.5 text-left text-sm transition-colors"
                classList={{
                  "bg-raised text-ink": index() === Math.min(cursor(), filtered().length - 1),
                  "text-ink-muted": index() !== Math.min(cursor(), filtered().length - 1),
                }}
                onMouseEnter={() => setCursor(index())}
                onClick={() => pick(item)}
              >
                <span class="min-w-0 flex-1 truncate">{item.label}</span>
                <span class="shrink-0 text-xs text-ink-faint">{item.hint}</span>
              </button>
            )}
          </For>
          <Show when={filtered().length === 0}>
            <div class="px-4 py-3 text-sm text-ink-faint">{t("palette.empty")}</div>
          </Show>
        </div>
      </div>
    </div>
  )
}
