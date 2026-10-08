import { mcpConfigFromForm, mcpFormState, updatePair, type McpFormState, type McpPair } from "../../state/mcp-form"
import { createSignal, Index, onCleanup, onMount, Show, untrack, type JSX, type Setter } from "solid-js"
import { activateModal, closeOnBackdropPointerDown } from "../modal"
import { IconPlus, IconX } from "../icons"
import { Portal } from "solid-js/web"
import { t } from "../../state/i18n"
import { Toggle } from "../controls"

import type { McpServerConfig, McpServerConfigView } from "../../engine/store"

/** The engine names a server's tools `<server>_<tool>`, so a name is what a tool name may hold. */
export const mcpServerName = /^[A-Za-z0-9_-]{1,128}$/

export function McpEditor(props: {
  server?: { name: string; config: McpServerConfigView; readOnlyTrusted: boolean }
  pending: boolean
  onClose: () => void
  onSave: (name: string, config: McpServerConfig, readOnlyTrusted: boolean) => Promise<void>
}) {
  let dialog!: HTMLDivElement
  const initialServer = untrack(() => props.server)
  const [name, setName] = createSignal(initialServer?.name ?? "")
  const [form, setForm] = createSignal(mcpFormState(initialServer?.config))
  const [trusted, setTrusted] = createSignal(initialServer?.readOnlyTrusted ?? true)
  const [error, setError] = createSignal("")
  const [submitting, setSubmitting] = createSignal(false)
  onMount(() => onCleanup(activateModal(dialog, props.onClose)))

  const save = async () => {
    if (submitting() || props.pending) return
    const serverName = name().trim()
    if (!serverName) return setError(t("drift.mcp.nameRequired"))
    if (!mcpServerName.test(serverName)) return setError(t("drift.mcp.form.nameInvalid"))
    const result = mcpConfigFromForm(form())
    if (result.issue) return setError(t(`drift.mcp.form.${result.issue}`))
    setSubmitting(true)
    setError("")
    try {
      await props.onSave(serverName, result.config, trusted())
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <Portal>
      <div
        data-modal-layer
        class="fixed inset-0 z-40 flex items-center justify-center bg-black/55 p-2 sm:p-4"
        onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose, dialog)}
      >
        <div
          ref={dialog}
          role="dialog"
          aria-modal="true"
          aria-label={props.server ? t("drift.mcp.edit") : t("drift.mcp.add")}
          tabIndex={-1}
          class="fade-up flex max-h-[calc(100vh-1rem)] w-[min(44rem,calc(100vw-1rem))] flex-col overflow-hidden rounded-xl border border-edge bg-overlay shadow-2xl"
        >
          <div class="flex items-center justify-between border-b border-edge px-4 py-3">
            <div class="text-sm font-semibold text-ink">{props.server ? t("drift.mcp.edit") : t("drift.mcp.add")}</div>
            <button
              title={t("common.close")}
              class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
              onClick={() => props.onClose()}
            >
              <IconX />
            </button>
          </div>
          <div class="min-h-0 flex-1 space-y-4 overflow-y-auto p-4">
            <Field label={t("drift.mcp.name")} required>
              <TextInput autofocus value={name()} onInput={setName} label={t("drift.mcp.name")} mono />
            </Field>
            <Field label={t("drift.mcp.form.type")} required>
              <div class="flex rounded-lg border border-edge bg-overlay/50 p-1">
                <Choice
                  active={form().type === "stdio"}
                  onClick={() => setForm((value) => ({ ...value, type: "stdio" }))}
                >
                  {t("drift.mcp.form.local")}
                </Choice>
                <Choice
                  active={form().type !== "stdio"}
                  onClick={() => setForm((value) => ({ ...value, type: value.type === "sse" ? "sse" : "http" }))}
                >
                  {t("drift.mcp.form.remote")}
                </Choice>
              </div>
            </Field>
            <Show when={form().type === "stdio"}>
              <LocalFields form={form()} setForm={setForm} />
            </Show>
            <Show when={form().type !== "stdio"}>
              <Field label={t("drift.mcp.form.transport")} required>
                <div class="flex rounded-lg border border-edge bg-overlay/50 p-1">
                  <Choice
                    active={form().type === "http"}
                    onClick={() => setForm((value) => ({ ...value, type: "http" }))}
                  >
                    {t("drift.mcp.transport.streamable_http")}
                  </Choice>
                  <Choice
                    active={form().type === "sse"}
                    onClick={() => setForm((value) => ({ ...value, type: "sse" }))}
                  >
                    {t("drift.mcp.transport.sse")}
                  </Choice>
                </div>
              </Field>
              <Field label={t("drift.mcp.form.url")} required>
                <TextInput
                  type="url"
                  value={form().url}
                  onInput={(url) => setForm((value) => ({ ...value, url }))}
                  label={t("drift.mcp.form.url")}
                  placeholder="https://example.com/mcp"
                  mono
                />
              </Field>
              <PairFields
                label={t("drift.mcp.form.headers")}
                pairs={form().headers}
                onChange={(headers) => setForm((value) => ({ ...value, headers }))}
              />
              <div class="rounded-md border border-edge/70 px-3 py-2">
                <div class="text-[0.78rem] font-medium text-ink">{t("drift.mcp.form.app")}</div>
                <div class="mt-2 space-y-3">
                  <div class="text-[0.7rem] text-ink-faint">{t("drift.mcp.form.appHint")}</div>
                  <Field label={t("drift.mcp.form.clientId")}>
                    <TextInput
                      value={form().clientId}
                      onInput={(clientId) =>
                        setForm((value) => ({
                          ...value,
                          clientId,
                          secretSaved: value.secretSaved && clientId.trim() === value.clientId.trim(),
                        }))
                      }
                      label={t("drift.mcp.form.clientId")}
                      mono
                    />
                  </Field>
                  <Field label={t("drift.mcp.form.clientSecret")}>
                    <TextInput
                      type="password"
                      value={form().clientSecret}
                      onInput={(clientSecret) => setForm((value) => ({ ...value, clientSecret }))}
                      label={t("drift.mcp.form.clientSecret")}
                      placeholder={t(
                        form().secretSaved ? "drift.mcp.form.savedValue" : "drift.mcp.form.clientSecretNone",
                      )}
                      mono
                    />
                  </Field>
                  <Field label={t("drift.mcp.form.scopes")}>
                    <TextInput
                      value={form().scopes}
                      onInput={(scopes) => setForm((value) => ({ ...value, scopes }))}
                      label={t("drift.mcp.form.scopes")}
                      placeholder={t("drift.mcp.form.scopesNone")}
                      mono
                    />
                  </Field>
                </div>
              </div>
            </Show>
            <Field label={t("drift.mcp.form.timeout")}>
              <TextInput
                value={form().timeout}
                onInput={(timeout) => setForm((value) => ({ ...value, timeout }))}
                label={t("drift.mcp.form.timeout")}
                placeholder={t("drift.mcp.form.timeoutNone")}
                mono
              />
            </Field>
            <div class="flex items-center gap-2.5 text-[0.78rem] text-ink">
              <Toggle
                label={t("drift.mcp.readOnlyTrusted")}
                checked={trusted()}
                onChange={() => setTrusted((value) => !value)}
              />
              <span>{t("drift.mcp.readOnlyTrusted")}</span>
            </div>
            <Show when={error()}>
              {(value) => (
                <div role="alert" class="text-xs text-danger">
                  {value()}
                </div>
              )}
            </Show>
          </div>
          <div class="flex justify-end gap-2 border-t border-edge px-4 py-3">
            <Button onClick={props.onClose}>{t("common.cancel")}</Button>
            <button
              class="h-8 rounded-md bg-accent px-3 text-xs font-medium text-accent-ink transition-opacity disabled:opacity-40"
              disabled={props.pending || submitting()}
              onClick={() => void save()}
            >
              {props.pending || submitting() ? t("common.loading") : t("common.save")}
            </button>
          </div>
        </div>
      </div>
    </Portal>
  )
}

function LocalFields(props: { form: McpFormState; setForm: Setter<McpFormState> }) {
  return (
    <div class="space-y-4">
      <Field label={t("drift.mcp.form.command")} required>
        <div class="space-y-2">
          <Index each={props.form.command}>
            {(part, index) => (
              <div class="flex gap-2">
                <TextInput
                  value={part()}
                  onInput={(text) => {
                    const command = [...props.form.command]
                    command[index] = text
                    props.setForm((value) => ({ ...value, command }))
                  }}
                  label={index ? t("drift.mcp.form.argument", { number: index }) : t("drift.mcp.form.executable")}
                  placeholder={index ? "--argument" : "npx"}
                  mono
                />
                <Show when={index > 0}>
                  <button
                    class="flex size-8 shrink-0 items-center justify-center rounded-md border border-edge text-ink-muted transition-colors hover:border-danger hover:text-danger"
                    title={t("drift.mcp.form.removeArgument")}
                    onClick={() =>
                      props.setForm((value) => ({
                        ...value,
                        command: value.command.filter((_, item) => item !== index),
                      }))
                    }
                  >
                    <IconX class="size-3.5" />
                  </button>
                </Show>
              </div>
            )}
          </Index>
          <AddButton
            label={t("drift.mcp.form.addArgument")}
            onClick={() => props.setForm((value) => ({ ...value, command: [...value.command, ""] }))}
          />
        </div>
      </Field>
      <PairFields
        label={t("drift.mcp.form.environment")}
        pairs={props.form.environment}
        onChange={(environment) => props.setForm((value) => ({ ...value, environment }))}
      />
      <Field label={t("drift.mcp.form.cwd")}>
        <TextInput
          value={props.form.cwd}
          onInput={(cwd) => props.setForm((value) => ({ ...value, cwd }))}
          label={t("drift.mcp.form.cwd")}
          placeholder={t("drift.mcp.form.cwdDefault")}
          mono
        />
      </Field>
    </div>
  )
}

function PairFields(props: { label: string; pairs: McpPair[]; onChange: (pairs: McpPair[]) => void }) {
  return (
    <Field label={props.label}>
      <div class="space-y-2">
        <Index each={props.pairs}>
          {(pair, index) => (
            <div class="flex gap-2">
              <TextInput
                value={pair().key}
                onInput={(key) => props.onChange(updatePair(props.pairs, index, { key }))}
                label={t("drift.mcp.form.key")}
                placeholder="NAME"
                mono
              />
              {/* Values are often keys or tokens: masked while typed, and a saved one is never shown at all. */}
              <TextInput
                type="password"
                value={pair().value}
                onInput={(value) => props.onChange(updatePair(props.pairs, index, { value }))}
                label={t("drift.mcp.form.value")}
                placeholder={pair().saved ? t("drift.mcp.form.savedValue") : undefined}
                mono
              />
              <button
                class="flex size-8 shrink-0 items-center justify-center rounded-md border border-edge text-ink-muted transition-colors hover:border-danger hover:text-danger"
                title={t("drift.mcp.form.removePair")}
                onClick={() => props.onChange(props.pairs.filter((_, item) => item !== index))}
              >
                <IconX class="size-3.5" />
              </button>
            </div>
          )}
        </Index>
        <AddButton
          label={t("drift.mcp.form.addPair")}
          onClick={() => props.onChange([...props.pairs, { key: "", value: "" }])}
        />
      </div>
    </Field>
  )
}

function Field(props: { label: string; required?: boolean; children: JSX.Element }) {
  return (
    <div class="text-xs text-ink-muted">
      <div class="text-[0.78rem] font-medium text-ink">
        {props.label}
        {/* Settings never uses danger red for anything but errors, so required reads as a hint. */}
        {props.required ? <span class="text-ink-faint"> *</span> : null}
      </div>
      <div class="mt-1.5">{props.children}</div>
    </div>
  )
}

function TextInput(props: {
  value: string
  onInput: (value: string) => void
  label: string
  type?: string
  placeholder?: string
  mono?: boolean
  autofocus?: boolean
}) {
  return (
    <input
      autofocus={props.autofocus}
      type={props.type ?? "text"}
      aria-label={props.label}
      class="h-8 w-full min-w-0 rounded-md border border-edge bg-raised/45 px-2.5 text-sm text-ink outline-none transition-colors placeholder:text-ink-faint focus:border-accent"
      classList={{ "font-mono text-xs": props.mono }}
      placeholder={props.placeholder}
      value={props.value}
      onInput={(event) => props.onInput(event.currentTarget.value)}
    />
  )
}

function Choice(props: { active: boolean; onClick: () => void; children: JSX.Element }) {
  return (
    <button
      type="button"
      aria-pressed={props.active}
      class="min-w-0 flex-1 rounded-md px-2.5 py-1 text-xs transition-colors"
      classList={{ "bg-raised text-ink": props.active, "text-ink-faint hover:text-ink": !props.active }}
      onClick={() => props.onClick()}
    >
      {props.children}
    </button>
  )
}

function Button(props: { onClick: () => void; children: JSX.Element }) {
  return (
    <button
      type="button"
      class="h-8 rounded-md border border-edge px-3 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
      onClick={() => props.onClick()}
    >
      {props.children}
    </button>
  )
}

function AddButton(props: { label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      class="flex h-8 items-center gap-1.5 rounded-md border border-edge px-2.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
      onClick={() => props.onClick()}
    >
      <IconPlus class="size-3.5" />
      {props.label}
    </button>
  )
}
