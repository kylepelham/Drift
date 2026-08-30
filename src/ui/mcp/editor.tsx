import { createSignal, Index, onCleanup, onMount, Show, type JSX, type Setter } from "solid-js"
import { Portal } from "solid-js/web"
import {
  mcpConfigFromForm,
  mcpFormState,
  withMcpOAuthPresence,
  withMcpPresence,
  type McpFormState,
  type McpPair,
} from "../../state/mcp-form"
import type { McpStoredExpectation } from "../../state/mcp"
import { t } from "../../state/i18n"
import type { McpConfig, StoredMcpServer } from "../../state/store"
import { IconPlus, IconX } from "../icons"
import { activateModal, closeOnBackdropPointerDown } from "../modal"
import { Toggle } from "../controls"

export function McpEditor(props: {
  server?: StoredMcpServer
  expected: McpStoredExpectation
  /** Config files defining an externally declared server; saving rewrites every one of them. */
  paths?: string[]
  pending: boolean
  onClose: () => void
  onSave: (name: string, config: McpConfig, expected: McpStoredExpectation) => Promise<void>
}) {
  let dialog!: HTMLDivElement
  const [name, setName] = createSignal(props.server?.name ?? "")
  const [form, setForm] = createSignal(mcpFormState(props.server?.config))
  const [error, setError] = createSignal("")
  const [submitting, setSubmitting] = createSignal(false)
  onMount(() => onCleanup(activateModal(dialog, props.onClose)))

  const save = async () => {
    if (submitting() || props.pending) return
    const serverName = name().trim()
    if (!serverName) return setError(t("drift.mcp.nameRequired"))
    if (serverName.length > 128 || !/^[A-Za-z0-9._/-]+$/.test(serverName)) {
      return setError(t("drift.mcp.form.nameInvalid"))
    }
    const result = mcpConfigFromForm(form())
    if (result.issue) return setError(t(`drift.mcp.form.${result.issue}`))
    setSubmitting(true)
    setError("")
    try {
      await props.onSave(serverName, result.config, props.expected)
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
              onClick={props.onClose}
            >
              <IconX />
            </button>
          </div>
          <div class="min-h-0 flex-1 space-y-4 overflow-y-auto p-4">
            <Show when={props.paths?.length}>
              <div class="rounded-md border border-edge bg-surface px-3 py-2 text-xs text-ink-faint">
                {t("drift.mcp.definedIn", { files: props.paths!.join(", ") })}
              </div>
            </Show>
            <Field label={t("drift.mcp.name")} required>
              <TextInput autofocus value={name()} onInput={setName} label={t("drift.mcp.name")} mono />
            </Field>
            <div class="grid gap-4 sm:grid-cols-2">
              <Field label={t("drift.mcp.form.type")} required>
                <div class="flex rounded-lg border border-edge bg-overlay/50 p-1">
                  <Choice
                    active={form().type === "local"}
                    onClick={() => setForm((value) => ({ ...value, type: "local" }))}
                  >
                    {t("drift.mcp.form.local")}
                  </Choice>
                  <Choice
                    active={form().type === "remote"}
                    onClick={() => setForm((value) => ({ ...value, type: "remote" }))}
                  >
                    {t("drift.mcp.form.remote")}
                  </Choice>
                </div>
              </Field>
              <Field label={t("drift.mcp.form.timeout")}>
                <TextInput
                  type="number"
                  value={form().timeout}
                  onInput={(timeout) =>
                    setForm((value) => withMcpPresence({ ...value, timeout }, "timeout", !!timeout))
                  }
                  label={t("drift.mcp.form.timeout")}
                  placeholder="5000"
                />
              </Field>
            </div>
            {/* Mirrors SettingsRow: label left, control right, hairline separators. */}
            <div class="flex min-h-13 items-center justify-between gap-4 border-y border-edge/70 px-1 py-2.5">
              <span class="text-[0.82rem] font-medium text-ink">{t("drift.mcp.form.enabled")}</span>
              <Toggle
                label={t("drift.mcp.form.enabled")}
                checked={form().enabled}
                onChange={() =>
                  setForm((value) => withMcpPresence({ ...value, enabled: !value.enabled }, "enabled", true))
                }
              />
            </div>
            <Show when={form().type === "local"}>
              <LocalFields form={form()} setForm={setForm} />
            </Show>
            <Show when={form().type === "remote"}>
              <RemoteFields form={form()} setForm={setForm} />
            </Show>
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
      <Field label={t("drift.mcp.form.cwd")}>
        <TextInput
          value={props.form.cwd}
          onInput={(cwd) => props.setForm((value) => withMcpPresence({ ...value, cwd }, "cwd", !!cwd))}
          label={t("drift.mcp.form.cwd")}
          placeholder="./tools"
          mono
        />
      </Field>
      <PairFields
        label={t("drift.mcp.form.environment")}
        pairs={props.form.environment}
        onChange={(environment) =>
          props.setForm((value) => withMcpPresence({ ...value, environment }, "environment", true))
        }
      />
    </div>
  )
}

function RemoteFields(props: { form: McpFormState; setForm: Setter<McpFormState> }) {
  return (
    <div class="space-y-4">
      <Field label={t("drift.mcp.form.url")} required>
        <TextInput
          type="url"
          value={props.form.url}
          onInput={(url) => props.setForm((value) => ({ ...value, url }))}
          label={t("drift.mcp.form.url")}
          placeholder="https://example.com/mcp"
          mono
        />
      </Field>
      <PairFields
        label={t("drift.mcp.form.headers")}
        pairs={props.form.headers}
        onChange={(headers) => props.setForm((value) => withMcpPresence({ ...value, headers }, "headers", true))}
      />
      <Field label={t("drift.mcp.form.oauth")}>
        <div class="flex rounded-lg border border-edge bg-overlay/50 p-1">
          <Choice
            active={props.form.oauthMode === "auto"}
            onClick={() => props.setForm((value) => ({ ...value, oauthMode: "auto" }))}
          >
            {t("drift.mcp.form.oauth.auto")}
          </Choice>
          <Choice
            active={props.form.oauthMode === "disabled"}
            onClick={() => props.setForm((value) => ({ ...value, oauthMode: "disabled" }))}
          >
            {t("drift.mcp.form.oauth.disabled")}
          </Choice>
          <Choice
            active={props.form.oauthMode === "configured"}
            onClick={() => props.setForm((value) => ({ ...value, oauthMode: "configured" }))}
          >
            {t("drift.mcp.form.oauth.configured")}
          </Choice>
        </div>
      </Field>
      <Show when={props.form.oauthMode === "configured"}>
        <div class="grid gap-4 border-l-2 border-edge pl-3 sm:grid-cols-2">
          <OAuthField field="clientId" label={t("drift.mcp.form.clientId")} form={props.form} setForm={props.setForm} />
          <OAuthField
            field="clientSecret"
            label={t("drift.mcp.form.clientSecret")}
            form={props.form}
            setForm={props.setForm}
            password
          />
          <OAuthField field="scope" label={t("drift.mcp.form.scope")} form={props.form} setForm={props.setForm} />
          <OAuthField
            field="callbackPort"
            label={t("drift.mcp.form.callbackPort")}
            form={props.form}
            setForm={props.setForm}
            number
          />
          <div class="sm:col-span-2">
            <OAuthField
              field="redirectUri"
              label={t("drift.mcp.form.redirectUri")}
              form={props.form}
              setForm={props.setForm}
            />
          </div>
        </div>
      </Show>
    </div>
  )
}

function OAuthField(props: {
  field: "clientId" | "clientSecret" | "scope" | "callbackPort" | "redirectUri"
  label: string
  form: McpFormState
  setForm: Setter<McpFormState>
  password?: boolean
  number?: boolean
}) {
  return (
    <Field label={props.label}>
      <TextInput
        type={props.password ? "password" : props.number ? "number" : "text"}
        value={props.form[props.field]}
        onInput={(text) =>
          props.setForm((value) => withMcpOAuthPresence({ ...value, [props.field]: text }, props.field, !!text))
        }
        label={props.label}
      />
    </Field>
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
              <TextInput
                value={pair().value}
                onInput={(value) => props.onChange(updatePair(props.pairs, index, { value }))}
                label={t("drift.mcp.form.value")}
                placeholder="{env:NAME}"
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

function updatePair(pairs: McpPair[], index: number, patch: Partial<McpPair>) {
  return pairs.map((pair, item) => (item === index ? { ...pair, ...patch } : pair))
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
      onClick={props.onClick}
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
      onClick={props.onClick}
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
      onClick={props.onClick}
    >
      <IconPlus class="size-3.5" />
      {props.label}
    </button>
  )
}
