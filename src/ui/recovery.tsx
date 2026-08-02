import { createEffect, createMemo, createSignal, Show } from "solid-js"
import { useEngine } from "../engine"
import { modelInfo, resolveModel, type EngineState, type ModelRef } from "../engine/store"
import { t } from "../state/i18n"
import { lmStudioModelReady } from "../state/lm-studio"
import { orderedModelProviderIds, prefsFor, updatePrefs } from "../state/prefs"
import type { RecoverableInterruption } from "../state/store"
import { Picker, type PickerItem } from "./picker"
import { ProviderIcon } from "./provider-icon"

export function recoveryModelItems(state: EngineState): PickerItem[] {
  const providers = state.providers.filter((provider) => {
    if (provider.id === "lmstudio") return state.connected.includes(provider.id)
    return state.connected.includes(provider.id) || state.connected.length === 0
  })
  return orderedModelProviderIds(providers.map((provider) => provider.id)).flatMap((providerID) => {
    const provider = providers.find((item) => item.id === providerID)
    if (!provider) return []
    return Object.values(provider.models)
      .filter((model) => (provider.id === "lmstudio" ? lmStudioModelReady(model) : model.capabilities.toolcall))
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((model) => ({
        id: `${provider.id}/${model.id}`,
        label: model.name,
        group: provider.name,
        providerID: provider.id,
        family: model.family,
        releaseDate: model.release_date,
      }))
  })
}

export function RecoveryCard(props: { interruption: RecoverableInterruption }) {
  const engine = useEngine()
  const failedModel = (): ModelRef => ({ providerID: props.interruption.providerId, modelID: props.interruption.modelId })
  const initial = () =>
    modelInfo(engine.state, failedModel()) ? failedModel() : resolveModel(engine.state, prefsFor(props.interruption.sessionId).model)
  const [selected, setSelected] = createSignal<ModelRef | null>(initial())
  const [submitting, setSubmitting] = createSignal(false)
  const items = createMemo(() => recoveryModelItems(engine.state))
  createEffect(() => {
    items()
    if (!selected()) setSelected(initial())
  })
  const selectedId = () => {
    const model = selected()
    return model ? `${model.providerID}/${model.modelID}` : undefined
  }
  const failedLabel = () =>
    modelInfo(engine.state, failedModel())?.name ?? `${props.interruption.providerId}/${props.interruption.modelId}`

  async function resume() {
    const model = selected()
    if (!model || submitting()) return
    setSubmitting(true)
    updatePrefs(props.interruption.sessionId, { model })
    const prefs = prefsFor(props.interruption.sessionId)
    const variants = Object.keys(modelInfo(engine.state, model)?.variants ?? {})
    const sessionAgent = (engine.state.sessions[props.interruption.sessionId] as { agent?: string } | undefined)?.agent
    await engine.actions.recover(props.interruption.sessionId, {
      model,
      agent: sessionAgent ?? prefs.agent,
      variant: prefs.variant && variants.includes(prefs.variant) ? prefs.variant : undefined,
    })
    setSubmitting(false)
  }

  return (
    <div class="rounded-xl border border-warn/45 bg-warn/8 px-4 py-3 text-sm" role="alert">
      <div class="font-semibold text-warn">{t("drift.recovery.title")}</div>
      <div class="mt-1 text-ink-muted">{t("drift.recovery.explanation")}</div>
      <div class="mt-2 rounded-md border border-edge bg-surface/60 px-3 py-2 text-ink-muted">
        <div>
          <span class="text-ink-faint">{t("drift.recovery.failedModel")}: </span>
          <span class="font-medium text-ink">{failedLabel()}</span>
        </div>
        <div class="mt-1 break-words">{props.interruption.reason}</div>
      </div>
      <div class="mt-3 flex flex-wrap items-center gap-2">
        <Picker
          label={t("drift.recovery.chooseModel")}
          items={items()}
          selected={selectedId()}
          fallbackLabel={selectedId()}
          icon={<ProviderIcon id={selected()?.providerID} class="size-3.5 shrink-0" />}
          bordered
          floating
          placement="above"
          onPick={(id) => {
            const [providerID, ...rest] = id.split("/")
            setSelected({ providerID, modelID: rest.join("/") })
          }}
        />
        <button
          class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-40"
          disabled={!selected() || submitting()}
          onClick={() => void resume()}
        >
          {submitting() ? t("drift.recovery.starting") : t("drift.recovery.continue")}
        </button>
      </div>
      <Show when={selected()}>
        <div class="mt-2 text-xs text-ink-faint">{t("drift.recovery.durableHint")}</div>
      </Show>
    </div>
  )
}
