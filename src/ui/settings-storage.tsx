import { For, onMount, Show } from "solid-js"
import { t } from "../state/i18n"
import {
  analyzeStorage,
  autoCleanup,
  compactStorage,
  formatBytes,
  pruneStorage,
  refreshStorageStats,
  setAutoCleanup,
  setStorageRule,
  storageBusy,
  storageError,
  storageEstimates,
  storageRules,
  storageStats,
  type StorageRules,
} from "../state/storage"
import { Toggle } from "./controls"
import { SettingsGroup, SettingsRow } from "./settings-controls"

/** One band of the usage bar. `tone` is a Tailwind background class. */
type Segment = { key: string; label: string; bytes: number; tone: string }

/**
 * The `event` log is listed first because it is almost always the largest consumer and the only one
 * this screen can actually shrink; `part` and `message` are the transcripts themselves.
 */
const tableTones: Record<string, { tone: string; label: string; hint: string }> = {
  event: { tone: "bg-accent", label: "drift.storage.table.event", hint: "drift.storage.table.event.hint" },
  part: { tone: "bg-ok", label: "drift.storage.table.part", hint: "drift.storage.table.part.hint" },
  message: { tone: "bg-warn", label: "drift.storage.table.message", hint: "drift.storage.table.message.hint" },
}

const ruleLabels: { rule: keyof StorageRules; label: string; description: string }[] = [
  {
    rule: "supersededSnapshots",
    label: "drift.storage.rule.superseded",
    description: "drift.storage.rule.superseded.description",
  },
  {
    rule: "subagentEvents",
    label: "drift.storage.rule.subagent",
    description: "drift.storage.rule.subagent.description",
  },
  {
    rule: "archivedEvents",
    label: "drift.storage.rule.archived",
    description: "drift.storage.rule.archived.description",
  },
  { rule: "orphanEvents", label: "drift.storage.rule.orphan", description: "drift.storage.rule.orphan.description" },
]

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

  /** Bar widths are relative to the file, so unaccounted bytes (indexes) simply leave a gap. */
  const barTotal = () => Math.max(stats()?.totalBytes ?? 0, 1)
  const reclaimable = () => storageEstimates()?.reduce((sum, estimate) => Math.max(sum, estimate.bytes), 0) ?? 0

  return (
    <div class="space-y-6">
      <Show
        when={stats()}
        fallback={
          <div class="px-2 text-sm text-ink-faint">
            {storageError() || t("common.loading")}
          </div>
        }
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
              <SettingsRow title={t("drift.storage.sessions.total")} description={t("drift.storage.sessions.total.description")}>
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

      <SettingsGroup title={t("drift.storage.cleanup")}>
        <SettingsRow title={t("drift.storage.auto")} description={t("drift.storage.auto.description")}>
          <Toggle label={t("drift.storage.auto")} checked={autoCleanup()} onChange={() => setAutoCleanup(!autoCleanup())} />
        </SettingsRow>
        <For each={ruleLabels}>
          {(entry) => (
            <SettingsRow title={t(entry.label)} description={t(entry.description)}>
              <div class="flex items-center gap-3">
                <Show when={ruleBytes(entry.rule) > 0}>
                  <span class="text-[0.7rem] tabular-nums text-ink-faint">{formatBytes(ruleBytes(entry.rule))}</span>
                </Show>
                <Toggle
                  label={t(entry.label)}
                  checked={storageRules()[entry.rule]}
                  onChange={() => setStorageRule(entry.rule, !storageRules()[entry.rule])}
                />
              </div>
            </SettingsRow>
          )}
        </For>
      </SettingsGroup>

      <SettingsGroup title={t("drift.storage.actions")}>
        <SettingsRow title={t("drift.storage.analyze")} description={t("drift.storage.analyze.description")}>
          <button
            class="h-9 rounded-md border border-edge px-3.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
            disabled={working()}
            onClick={() => void analyzeStorage()}
          >
            {storageBusy() === "analyze" ? t("drift.storage.analyzing") : t("drift.storage.analyze.action")}
          </button>
        </SettingsRow>
        <SettingsRow
          title={t("drift.storage.prune")}
          description={
            reclaimable() > 0
              ? t("drift.storage.prune.available", { size: formatBytes(reclaimable()) })
              : t("drift.storage.prune.description")
          }
        >
          <button
            class="h-9 rounded-md bg-accent px-3.5 text-xs font-medium text-accent-ink transition-colors hover:brightness-105 disabled:opacity-40"
            disabled={working() || !Object.values(storageRules()).some(Boolean)}
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

/** Rule names from the backend are prefixed (`superseded:message.part.updated.1`). */
function ruleKey(rule: keyof StorageRules) {
  if (rule === "supersededSnapshots") return "superseded"
  if (rule === "subagentEvents") return "subagent-events"
  if (rule === "archivedEvents") return "archived-events"
  return "orphan-events"
}

/** Superseded snapshots are reported per event type, so its estimates are summed. */
function ruleBytes(rule: keyof StorageRules) {
  const key = ruleKey(rule)
  return (storageEstimates() ?? [])
    .filter((estimate) => estimate.rule.startsWith(key))
    .reduce((sum, estimate) => sum + estimate.bytes, 0)
}
