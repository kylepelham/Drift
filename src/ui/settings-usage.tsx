import { createEffect, createMemo, For, Show } from "solid-js"
import { useEngine } from "../engine"
import { t } from "../state/i18n"
import { planLabel, refreshUsage, usageFor } from "../state/usage-limits"
import { LimitRow, usageMessage } from "./context-meter"
import { ProviderIcon } from "./provider-icon"

export function UsageLimitsSection() {
  const engine = useEngine()
  const linked = createMemo(() =>
    engine.state.providers.filter((provider) => engine.state.connected.includes(provider.id)).sort((a, b) => a.name.localeCompare(b.name)),
  )
  createEffect(() => {
    for (const provider of linked()) void refreshUsage(provider.id)
  })
  // Providers without plan windows answer null; they are left out rather than listed as empty.
  const reporting = () => linked().filter((provider) => usageFor(provider.id)?.usage !== null)
  const loading = () => linked().some((provider) => !usageFor(provider.id) || usageFor(provider.id)?.loading)
  const refreshAll = () => {
    for (const provider of linked()) void refreshUsage(provider.id, Date.now(), true)
  }
  return (
    <div class="space-y-3">
      <div class="flex items-center justify-between gap-4">
        <p class="text-[0.72rem] leading-relaxed text-ink-faint">{t("drift.usage.settingsDescription")}</p>
        <button
          class="h-8 shrink-0 rounded-md border border-edge px-3 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
          disabled={loading()}
          onClick={refreshAll}
        >
          {t("drift.usage.refresh")}
        </button>
      </div>
      <Show when={reporting().length || loading()} fallback={<p class="text-xs text-ink-faint">{t("drift.usage.none")}</p>}>
        <div class="space-y-2" data-usage-settings>
          <For each={reporting()}>
            {(provider) => {
              const entry = () => usageFor(provider.id)
              return (
                <section class="space-y-2.5 rounded-xl border border-edge bg-raised/25 px-3 py-3 text-xs">
                  <div class="flex items-center gap-2.5">
                    <span class="flex size-7 shrink-0 items-center justify-center rounded-lg border border-edge bg-surface text-ink-muted">
                      <ProviderIcon id={provider.id} class="size-4" />
                    </span>
                    <span class="min-w-0 flex-1 truncate text-sm font-medium text-ink">{provider.name}</span>
                    <Show when={entry()?.usage?.plan}>
                      {(plan) => (
                        <span class="shrink-0 rounded-full border border-edge px-2 py-0.5 text-[0.68rem] text-ink-muted">{planLabel(plan())}</span>
                      )}
                    </Show>
                  </div>
                  <Show
                    when={usageMessage(entry())}
                    fallback={<For each={entry()?.usage?.windows}>{(window) => <LimitRow window={window} />}</For>}
                  >
                    {(message) => <div class="text-ink-faint">{message()}</div>}
                  </Show>
                </section>
              )
            }}
          </For>
        </div>
      </Show>
    </div>
  )
}
