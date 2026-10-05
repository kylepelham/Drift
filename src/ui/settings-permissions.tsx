import { createEffect, createSignal, For, on, onMount, Show } from "solid-js"
import { useEngine } from "../engine"
import type { PermissionGrant, PermissionRule } from "../engine/native/client"
import { t } from "../state/i18n"
import { activeWorkspace } from "../state/workspaces"
import { IconArrowDown, IconArrowUp, IconPlus, IconTrash } from "./icons"
import { Picker } from "./picker"
import { SettingsGroup } from "./settings-controls"

/** The kinds tools ask with; `*` covers them all. */
export const permissionKinds = ["*", "bash", "edit", "read", "glob", "grep", "webfetch", "mcp", "skill", "task", "project-commands"] as const
const decisions = ["allow", "ask", "deny"] as const

/** `rules` with the rule at `index` moved `by` places, kept inside the list. */
export function moveRule(rules: PermissionRule[], index: number, by: number) {
  const to = Math.min(Math.max(index + by, 0), rules.length - 1)
  const next = [...rules]
  next.splice(to, 0, ...next.splice(index, 1))
  return next
}

/** What a grant covers, as the user approved it. */
export function grantLabel(grant: PermissionGrant) {
  if (grant.grant === "exact") return `${grant.kind}: ${grant.target}`
  if (grant.grant === "subcommand") return `bash: ${t("drift.permissions.grant.subcommand", { prefix: grant.prefix })}`
  if (grant.grant === "folder") return `${grant.kind}: ${t("drift.permissions.grant.folder", { folder: grant.folder })}`
  return `${grant.kind}: ${grant.pattern}`
}

export function PermissionsSection() {
  return (
    <div class="space-y-6">
      <RulesGroup />
      <GrantsGroup />
    </div>
  )
}

function RulesGroup() {
  const engine = useEngine()
  const [saved, setSaved] = createSignal<PermissionRule[]>([])
  const [rules, setRules] = createSignal<PermissionRule[]>([])
  const [error, setError] = createSignal("")
  const [notice, setNotice] = createSignal(false)
  const [busy, setBusy] = createSignal(false)
  const dirty = () => JSON.stringify(rules()) !== JSON.stringify(saved())
  const update = (index: number, change: Partial<PermissionRule>) => {
    setNotice(false)
    setRules((list) => list.map((rule, at) => (at === index ? { ...rule, ...change } : rule)))
  }

  async function run(action: () => Promise<PermissionRule[]>, announce: boolean) {
    setBusy(true)
    setError("")
    try {
      const next = await action()
      setSaved(next)
      setRules(next)
      setNotice(announce)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  onMount(() => void run(() => engine.actions.permissionRules(), false))

  return (
    <SettingsGroup title={t("drift.permissions.rules")}>
      <div class="space-y-3 py-3">
        <div class="text-xs text-ink-faint">{t("drift.permissions.rulesDescription")}</div>
        <Show when={rules().length > 0} fallback={<div class="text-xs text-ink-faint">{t("drift.permissions.empty")}</div>}>
          <div class="space-y-1.5">
            <For each={rules()}>
              {(rule, index) => (
                <div class="flex items-center gap-2">
                  <Picker
                    label={t("drift.permissions.kind")}
                    items={permissionKinds.map((kind) => ({ id: kind, label: kind === "*" ? t("drift.permissions.kind.all") : kind }))}
                    selected={rule.kind}
                    fallbackLabel={rule.kind}
                    floating bordered chevronAtEnd placement="below" width="9.5rem"
                    onPick={(kind) => update(index(), { kind })}
                  />
                  <input
                    aria-label={t("drift.permissions.pattern")}
                    class="h-8 min-w-0 flex-1 rounded-md border border-edge bg-raised/45 px-2.5 font-mono text-xs text-ink outline-none transition-colors placeholder:text-ink-faint focus:border-accent"
                    placeholder="git push*"
                    value={rule.pattern}
                    onInput={(event) => update(index(), { pattern: event.currentTarget.value })}
                  />
                  <Picker
                    label={t("drift.permissions.decision")}
                    items={decisions.map((decision) => ({ id: decision, label: t(`drift.permissions.decision.${decision}`) }))}
                    selected={rule.decision}
                    floating bordered chevronAtEnd placement="below" width="6.5rem"
                    onPick={(decision) => update(index(), { decision: decision as PermissionRule["decision"] })}
                  />
                  <RowButton title={t("drift.permissions.moveUp")} disabled={index() === 0} onClick={() => setRules((list) => moveRule(list, index(), -1))}>
                    <IconArrowUp class="size-3.5" />
                  </RowButton>
                  <RowButton title={t("drift.permissions.moveDown")} disabled={index() === rules().length - 1} onClick={() => setRules((list) => moveRule(list, index(), 1))}>
                    <IconArrowDown class="size-3.5" />
                  </RowButton>
                  <RowButton title={t("drift.permissions.remove")} onClick={() => setRules((list) => list.filter((_, at) => at !== index()))}>
                    <IconTrash class="size-3.5" />
                  </RowButton>
                </div>
              )}
            </For>
          </div>
        </Show>
        <Show when={error()}>
          <div role="alert" class="text-xs text-danger">{error()}</div>
        </Show>
        <Show when={notice()}>
          <div role="status" class="text-xs text-ok">{t("drift.permissions.saved")}</div>
        </Show>
        <div class="flex items-center justify-between gap-2">
          <button
            class="flex h-8 items-center gap-1.5 rounded-md border border-edge px-2.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
            disabled={busy()}
            onClick={() => setRules((list) => [...list, { kind: "bash", pattern: "", decision: "ask" }])}
          >
            <IconPlus class="size-3.5" />
            {t("drift.permissions.add")}
          </button>
          <div class="flex gap-2">
            <Show when={dirty()}>
              <button
                class="rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
                onClick={() => {
                  setError("")
                  setRules(saved())
                }}
              >
                {t("common.reset")}
              </button>
            </Show>
            <button
              class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-40"
              disabled={busy() || !dirty()}
              onClick={() => void run(() => engine.actions.savePermissionRules(rules()), true)}
            >
              {t("common.save")}
            </button>
          </div>
        </div>
      </div>
    </SettingsGroup>
  )
}

function GrantsGroup() {
  const engine = useEngine()
  const [grants, setGrants] = createSignal<PermissionGrant[]>([])
  const [error, setError] = createSignal("")
  const [busy, setBusy] = createSignal(false)
  const directory = () => activeWorkspace()?.path

  async function run(action: () => Promise<unknown>) {
    const folder = directory()
    if (!folder) return setGrants([])
    setBusy(true)
    setError("")
    try {
      await action()
      setGrants(await engine.actions.workspaceGrants(folder))
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  createEffect(on(directory, () => void run(async () => undefined)))

  return (
    <SettingsGroup title={t("drift.permissions.grants")}>
      <div class="space-y-3 py-3">
        <Show when={activeWorkspace()} fallback={<div class="text-xs text-ink-faint">{t("drift.permissions.noWorkspace")}</div>}>
          {(workspace) => (
            <>
              <div class="text-xs text-ink-faint">{t("drift.permissions.grantsDescription", { workspace: workspace().name })}</div>
              <Show when={grants().length > 0} fallback={<div class="text-xs text-ink-faint">{t("drift.permissions.noGrants")}</div>}>
                <div class="space-y-1">
                  <For each={grants()}>
                    {(grant) => (
                      <div class="flex items-center gap-2 rounded-md px-2 py-1 hover:bg-raised/40">
                        <span class="min-w-0 flex-1 truncate font-mono text-xs text-ink" title={grantLabel(grant)}>
                          {grantLabel(grant)}
                        </span>
                        <button
                          class="rounded-md border border-edge px-2 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
                          disabled={busy()}
                          onClick={() => void run(() => engine.actions.revokeGrant(workspace().path, grant))}
                        >
                          {t("drift.permissions.revoke")}
                        </button>
                      </div>
                    )}
                  </For>
                </div>
                <div class="flex justify-end">
                  <button
                    class="rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
                    disabled={busy()}
                    onClick={() => void run(() => engine.actions.revokeGrant(workspace().path))}
                  >
                    {t("drift.permissions.revokeAll")}
                  </button>
                </div>
              </Show>
            </>
          )}
        </Show>
        <Show when={error()}>
          <div role="alert" class="text-xs text-danger">{error()}</div>
        </Show>
      </div>
    </SettingsGroup>
  )
}

function RowButton(props: { title: string; disabled?: boolean; onClick: () => void; children: import("solid-js").JSX.Element }) {
  return (
    <button
      type="button"
      title={props.title}
      aria-label={props.title}
      disabled={props.disabled}
      class="flex size-8 shrink-0 items-center justify-center rounded-md border border-edge text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-30"
      onClick={props.onClick}
    >
      {props.children}
    </button>
  )
}
