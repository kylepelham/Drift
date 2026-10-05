import { createEffect, createSignal, on, onMount, Show } from "solid-js"
import { useEngine } from "../engine"
import type { components } from "../engine/native/types"
import { t } from "../state/i18n"
import { Picker } from "./picker"
import { SettingsGroup } from "./settings-controls"

type BasePrompts = components["schemas"]["BasePrompts"]

const familyLabels: Record<string, string> = {
  all: "drift.settings.prompts.family.all",
  codex: "drift.settings.prompts.family.codex",
  claude: "drift.settings.prompts.family.claude",
  gemini: "drift.settings.prompts.family.gemini",
  default: "drift.settings.prompts.family.default",
}

/** The base prompt each model family starts with; a replacement applies from each conversation's next turn. */
export function BasePromptsSection() {
  const engine = useEngine()
  const [data, setData] = createSignal<BasePrompts | null>(null)
  const [selected, setSelected] = createSignal("all")
  const [draft, setDraft] = createSignal("")
  const [error, setError] = createSignal("")
  const [saving, setSaving] = createSignal(false)
  const [saved, setSaved] = createSignal(false)
  const current = () => data()?.prompts.find((prompt) => prompt.id === selected())
  const baseline = () => current()?.custom ?? current()?.default ?? ""
  const dirty = () => draft() !== baseline()

  onMount(() => void run(() => engine.actions.basePrompts(), false))
  createEffect(on([selected, data], () => setDraft(baseline())))

  async function run(action: () => Promise<BasePrompts>, announce = true) {
    setSaving(true)
    setError("")
    setSaved(false)
    try {
      setData(await action())
      setSaved(announce)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setSaving(false)
    }
  }

  function pick(id: string) {
    if (dirty()) return setError(t("drift.settings.prompts.saveBeforeSwitch"))
    setError("")
    setSelected(id)
  }

  return (
    <SettingsGroup title={t("drift.settings.prompts.modelFamilies")}>
      <div class="space-y-3 py-3">
        <div class="flex items-center justify-between gap-3">
          <div class="text-xs text-ink-faint">{t(selected() === "all" ? "drift.settings.prompts.allDescription" : "drift.settings.prompts.familyDescription")}</div>
          <Picker
            label={t("drift.settings.prompts.modelFamilies")}
            items={(data()?.prompts ?? []).map((prompt) => ({ id: prompt.id, label: t(familyLabels[prompt.id] ?? prompt.id) }))}
            selected={selected()}
            floating bordered chevronAtEnd placement="below" width="11rem"
            onPick={pick}
          />
        </div>
        <textarea
          aria-label={t("drift.settings.prompts.systemPrompt")}
          class="h-64 w-full resize-y rounded-lg border border-edge bg-bg/50 p-3 font-mono text-xs leading-relaxed outline-none transition-colors focus:border-accent"
          classList={{ "text-ink": !!current()?.custom || dirty(), "text-ink-faint": !current()?.custom && !dirty() }}
          spellcheck={false}
          placeholder={selected() === "all" ? t("drift.settings.prompts.allPlaceholder") : undefined}
          value={draft()}
          disabled={!data()}
          onInput={(event) => {
            setSaved(false)
            setDraft(event.currentTarget.value)
          }}
        />
        <details class="text-xs text-ink-faint">
          <summary class="cursor-pointer select-none">{t("drift.settings.prompts.sharedRules")}</summary>
          <p class="mt-2">{t("drift.settings.prompts.sharedDescription")}</p>
          <pre class="mt-2 max-h-48 overflow-auto whitespace-pre-wrap rounded-lg bg-bg/40 p-3 font-mono text-[0.68rem] leading-relaxed">{data()?.shared}</pre>
        </details>
        <Show when={error()}>
          <div role="alert" class="text-xs text-danger">{error()}</div>
        </Show>
        <Show when={saved()}>
          <div role="status" class="text-xs text-ok">{t("drift.settings.prompts.saved")}</div>
        </Show>
        <div class="flex justify-end gap-2">
          <Show when={current()?.custom !== undefined || dirty()}>
            <button
              class="rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
              disabled={saving()}
              onClick={() => (current()?.custom === undefined ? setDraft(baseline()) : void run(() => engine.actions.resetBasePrompt(selected())))}
            >
              {t("common.reset")}
            </button>
          </Show>
          <button
            class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-40"
            disabled={saving() || !dirty() || !draft().trim()}
            onClick={() => void run(() => engine.actions.saveBasePrompt(selected(), draft()))}
          >
            {t("common.save")}
          </button>
        </div>
      </div>
    </SettingsGroup>
  )
}
