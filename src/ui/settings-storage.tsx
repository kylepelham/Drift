import { SettingsGroup, SettingsRow } from "./settings-controls"
import { For, onMount, Show } from "solid-js"
import { t } from "../state/i18n"
import {
  compactStorage,
  formatBytes,
  pruneStorage,
  refreshStorageStats,
  storageBusy,
  storageError,
  storageStats,
} from "../state/storage"

/** One band of the usage bar. `tone` is a Tailwind background class. */
type Segment = { key: string; label: string; bytes: number; tone: string }

/** Transcripts and images live in the database; undo history and shell output in folders beside it. */
const tableTones: Record<string, { tone: string; label: string; hint: string }> = {
  part: { tone: "bg-ok", label: "drift.storage.table.part", hint: "drift.storage.table.part.hint" },
  blob: { tone: "bg-warn", label: "drift.storage.table.blob", hint: "drift.storage.table.blob.hint" },
  undo: { tone: "bg-accent", label: "drift.storage.table.undo", hint: "drift.storage.table.undo.hint" },
  output: { tone: "bg-ink-faint", label: "drift.storage.table.output", hint: "drift.storage.table.output.hint" },
}

export function StorageSection() {
  onMount(() => void refreshStorageStats())

  const stats = storageStats
  const working = () => storageBusy() !== null

  /** Table bands plus a trailing band for space already free inside the file. */
  const segments = (): Segment[] => {
    const current = stats()
    if (!current) return []
    const bands = current.tables
      .filter((table) => table.bytes > 0)
      .map((table) => ({
        key: table.table,
        label: t(tableTones[table.table]?.label ?? table.table),
        bytes: table.bytes,
        tone: tableTones[table.table]?.tone ?? "bg-ink-faint",
      }))
    if (current.freeBytes > 0) {
      bands.push({
        key: "free",
        label: t("drift.storage.free"),
        bytes: current.freeBytes,
        tone: "bg-edge-strong",
      })
    }
    return bands
  }

  /** Bar widths are relative to the whole, so unaccounted bytes (indexes) simply leave a gap. */
  const barTotal = () => Math.max(stats()?.totalBytes ?? 0, 1)

  return (
    <div class="space-y-6">
      <Show
        when={stats()}
        fallback={<div class="px-2 text-sm text-ink-faint">{storageError() || t("common.loading")}</div>}
      >
        {(current) => (
          <>
            <section>
              <div class="mb-3 flex items-baseline justify-between gap-3">
                <div class="min-w-0">
                  <div class="text-2xl font-semibold text-ink">{formatBytes(current().totalBytes)}</div>
                  <div class="mt-0.5 truncate text-[0.7rem] text-ink-faint" title={current().path}>
                    {t("drift.storage.subtitle")}
                  </div>
                </div>
                <button
                  class="h-8 shrink-0 rounded-md border border-edge px-3 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
                  disabled={working()}
                  onClick={() => void refreshStorageStats()}
                >
                  {t("drift.storage.refresh")}
                </button>
              </div>

              <div class="flex h-3 w-full overflow-hidden rounded-full bg-raised">
                <For each={segments()}>
                  {(segment) => (
                    <div
                      class={`h-full ${segment.tone}`}
                      style={{ width: `${(segment.bytes / barTotal()) * 100}%` }}
                      title={`${segment.label} - ${formatBytes(segment.bytes)}`}
                    />
                  )}
                </For>
              </div>

              <div class="mt-3 space-y-1.5">
                <For each={segments()}>
                  {(segment) => (
                    <div class="flex items-center gap-2.5">
                      <span class={`size-2.5 shrink-0 rounded-full ${segment.tone}`} />
                      <span class="min-w-0 flex-1 truncate text-[0.8rem] text-ink">{segment.label}</span>
                      <Show when={tableTones[segment.key]}>
                        <span class="hidden shrink-0 text-[0.7rem] text-ink-faint sm:inline">
                          {t(tableTones[segment.key].hint)}
                        </span>
                      </Show>
                      <span class="shrink-0 text-[0.8rem] tabular-nums text-ink-muted">
                        {formatBytes(segment.bytes)}
                      </span>
                    </div>
                  )}
                </For>
              </div>
              <Show when={current().estimated}>
                <div class="mt-2 text-[0.68rem] text-ink-faint">{t("drift.storage.estimated")}</div>
              </Show>
            </section>

            <SettingsGroup title={t("drift.storage.sessions")}>
              <SettingsRow
                title={t("drift.storage.sessions.total")}
                description={t("drift.storage.sessions.total.description")}
              >
                <span class="text-sm tabular-nums text-ink-muted">{current().sessions.total}</span>
              </SettingsRow>
              <SettingsRow
                title={t("drift.storage.sessions.subagent")}
                description={t("drift.storage.sessions.subagent.description")}
              >
                <span class="text-sm tabular-nums text-ink-muted">{current().sessions.subagent}</span>
              </SettingsRow>
              <SettingsRow
                title={t("drift.storage.sessions.archived")}
                description={t("drift.storage.sessions.archived.description")}
              >
                <span class="text-sm tabular-nums text-ink-muted">{current().sessions.archived}</span>
              </SettingsRow>
            </SettingsGroup>
          </>
        )}
      </Show>

      <SettingsGroup title={t("drift.storage.actions")}>
        <SettingsRow title={t("drift.storage.prune")} description={t("drift.storage.prune.description")}>
          <button
            class="h-9 rounded-md bg-accent px-3.5 text-xs font-medium text-accent-ink transition-colors hover:brightness-105 disabled:opacity-40"
            disabled={working()}
            onClick={() => void pruneStorage()}
          >
            {storageBusy() === "prune" ? t("drift.storage.pruning") : t("drift.storage.prune.action")}
          </button>
        </SettingsRow>
        <SettingsRow title={t("drift.storage.compact")} description={t("drift.storage.compact.description")}>
          <button
            class="h-9 rounded-md border border-edge px-3.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
            disabled={working()}
            onClick={() => void compactStorage()}
          >
            {storageBusy() === "compact" ? t("drift.storage.compacting") : t("drift.storage.compact.action")}
          </button>
        </SettingsRow>
      </SettingsGroup>

      <Show when={storageError()}>
        <div class="text-xs text-danger">{storageError()}</div>
      </Show>
    </div>
  )
}
