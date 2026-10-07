import { createEffect, createMemo, createSignal, For, on, onMount, Show } from "solid-js"
import { useEngine } from "../engine"
import type { PermissionGrant, PermissionRule } from "../engine/native/client"
import { t } from "../state/i18n"
import { activeWorkspace, workspaces } from "../state/workspaces"
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
        <RuleList rules={rules()} onChange={(next) => {
          setNotice(false)
          setRules(next)
        }} />
        <Show when={error()}>
          <div role="alert" class="text-xs text-danger">{error()}</div>
        </Show>
        <Show when={notice()}>
          <div role="status" class="text-xs text-ok">{t("drift.permissions.saved")}</div>
        </Show>
        <div class="flex items-center justify-between gap-2">
          <AddRule disabled={busy()} onAdd={() => setRules((list) => [...list, newRule()])} />
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

/** Which part of the page a grant belongs to: by what it lets through, not by the tool that asked. */
export function grantGroup(grant: PermissionGrant): "shell" | "files" | "web" | "mcp" | "other" {
  const kind = grant.grant === "subcommand" ? "bash" : grant.kind
  if (kind === "bash") return "shell"
  if (["read", "edit", "glob", "grep"].includes(kind)) return "files"
  if (kind === "webfetch") return "web"
  if (kind === "mcp") return "mcp"
  return "other"
}

/** What a grant covers, without the kind its group already names. */
export function grantText(grant: PermissionGrant) {
  if (grant.grant === "exact") return grant.target
  if (grant.grant === "subcommand") return t("drift.permissions.grant.subcommand", { prefix: grant.prefix })
  if (grant.grant === "folder") return t("drift.permissions.grant.folder", { folder: grant.folder })
  return grant.pattern
}

const grantGroups = ["shell", "files", "web", "mcp", "other"] as const
const filterFrom = 8

function GrantsGroup() {
  const engine = useEngine()
  const [chosen, setChosen] = createSignal<string>()
  const [grants, setGrants] = createSignal<PermissionGrant[]>([])
  const [filter, setFilter] = createSignal("")
  const [error, setError] = createSignal("")
  const [busy, setBusy] = createSignal(false)
  const workspace = () => workspaces().find((item) => item.id === chosen()) ?? activeWorkspace() ?? workspaces()[0]
  const directory = () => workspace()?.path

  async function run(action: () => Promise<unknown>) {
    const folder = directory()
    if (!folder) return
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

  createEffect(on(directory, () => {
    setFilter("")
    void run(async () => undefined)
  }))

  const shown = createMemo(() => {
    const words = filter().trim().toLowerCase()
    const matching = grants().filter((grant) => !words || `${grant.grant === "subcommand" ? "bash" : grant.kind} ${grantText(grant)}`.toLowerCase().includes(words))
    return grantGroups.map((group) => ({ group, grants: matching.filter((grant) => grantGroup(grant) === group) })).filter((entry) => entry.grants.length)
  })

  return (
    <section>
      <div class="mb-1.5 flex items-center justify-between gap-3">
        <div class="text-[0.68rem] font-semibold tracking-wide text-ink-faint uppercase">{t("drift.permissions.grants")}</div>
        <Show when={workspaces().length}>
          <Picker
            label={t("drift.permissions.workspace")}
            items={workspaces().map((item) => ({ id: item.id, label: item.name, hint: item.path }))}
            selected={workspace()?.id}
            floating bordered chevronAtEnd placement="below" width="13rem"
            onPick={setChosen}
          />
        </Show>
      </div>
      <div class="space-y-4 border-y border-edge/80 py-3">
        <Show when={workspace()} fallback={<div class="text-xs text-ink-faint">{t("drift.permissions.noWorkspace")}</div>}>
          {(current) => (
            <>
              <div class="flex items-start justify-between gap-3">
                <div class="text-xs leading-relaxed text-ink-faint">{t("drift.permissions.grantsDescription", { workspace: current().name })}</div>
                <Show when={grants().length}>
                  <button
                    class="shrink-0 rounded-md border border-edge px-2.5 py-1 text-xs text-ink-muted transition-colors hover:border-danger/50 hover:text-danger disabled:opacity-40"
                    disabled={busy()}
                    onClick={() => void run(() => engine.actions.revokeGrant(current().path))}
                  >
                    {t("drift.permissions.revokeAll")}
                  </button>
                </Show>
              </div>
              <Show when={grants().length >= filterFrom}>
                <input
                  class="h-8 w-full rounded-md border border-edge bg-raised/45 px-2.5 text-xs text-ink outline-none transition-colors placeholder:text-ink-faint focus:border-accent"
                  placeholder={t("drift.permissions.filter")}
                  aria-label={t("drift.permissions.filter")}
                  value={filter()}
                  onInput={(event) => setFilter(event.currentTarget.value)}
                />
              </Show>
              <Show when={grants().length} fallback={<div class="text-xs text-ink-faint">{t("drift.permissions.noGrants")}</div>}>
                <For each={shown()}>
                  {(entry) => (
                    <div>
                      <div class="mb-1 flex items-center gap-2 text-[0.72rem] font-medium text-ink-muted">
                        {t(`drift.permissions.group.${entry.group}`)}
                        <span class="text-ink-faint">{entry.grants.length}</span>
                      </div>
                      <div class="overflow-hidden rounded-md border border-edge/70">
                        <For each={entry.grants}>
                          {(grant) => (
                            <div class="group/grant flex items-center gap-2 border-b border-edge/50 px-2.5 py-1.5 last:border-b-0 hover:bg-raised/40">
                              <Show when={entry.group === "other" || entry.group === "files"}>
                                <span class="shrink-0 rounded bg-raised px-1.5 py-0.5 text-[0.65rem] text-ink-faint">{grant.grant === "subcommand" ? "bash" : grant.kind}</span>
                              </Show>
                              <span class="min-w-0 flex-1 truncate font-mono text-[0.72rem] text-ink" title={grantText(grant)}>{grantText(grant)}</span>
                              <button
                                type="button"
                                title={t("drift.permissions.revoke")}
                                aria-label={t("drift.permissions.revoke")}
                                class="flex size-6 shrink-0 items-center justify-center rounded text-ink-faint opacity-0 transition-opacity group-hover/grant:opacity-100 hover:bg-danger/10 hover:text-danger focus-visible:opacity-100 disabled:opacity-30"
                                disabled={busy()}
                                onClick={() => void run(() => engine.actions.revokeGrant(current().path, grant))}
                              >
                                <IconTrash class="size-3.5" />
                              </button>
                            </div>
                          )}
                        </For>
                      </div>
                    </div>
                  )}
                </For>
                <Show when={!shown().length}>
                  <div class="text-xs text-ink-faint">{t("drift.permissions.noMatch")}</div>
                </Show>
              </Show>
            </>
          )}
        </Show>
        <Show when={error()}>
          <div role="alert" class="text-xs text-danger">{error()}</div>
        </Show>
      </div>
    </section>
  )
}

export const newRule = (): PermissionRule => ({ kind: "bash", pattern: "", decision: "ask" })

/** Rules as editable rows: kind, pattern, decision, and moving or removing each. */
export function RuleList(props: { rules: PermissionRule[]; onChange: (rules: PermissionRule[]) => void }) {
  const update = (index: number, change: Partial<PermissionRule>) => props.onChange(props.rules.map((rule, at) => (at === index ? { ...rule, ...change } : rule)))
  return (
    <Show when={props.rules.length > 0} fallback={<div class="text-xs text-ink-faint">{t("drift.permissions.empty")}</div>}>
      <div class="space-y-1.5">
        <For each={props.rules}>
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
              <RowButton title={t("drift.permissions.moveUp")} disabled={index() === 0} onClick={() => props.onChange(moveRule(props.rules, index(), -1))}>
                <IconArrowUp class="size-3.5" />
              </RowButton>
              <RowButton title={t("drift.permissions.moveDown")} disabled={index() === props.rules.length - 1} onClick={() => props.onChange(moveRule(props.rules, index(), 1))}>
                <IconArrowDown class="size-3.5" />
              </RowButton>
              <RowButton title={t("drift.permissions.remove")} onClick={() => props.onChange(props.rules.filter((_, at) => at !== index()))}>
                <IconTrash class="size-3.5" />
              </RowButton>
            </div>
          )}
        </For>
      </div>
    </Show>
  )
}

export function AddRule(props: { disabled?: boolean; onAdd: () => void }) {
  return (
    <button
      class="flex h-8 items-center gap-1.5 rounded-md border border-edge px-2.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
      disabled={props.disabled}
      onClick={props.onAdd}
    >
      <IconPlus class="size-3.5" />
      {t("drift.permissions.add")}
    </button>
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
