import { createMemo, createSignal, For, Show } from "solid-js"
import { useEngine } from "../engine"
import { taskActive, type TaskRecord } from "../engine/store"
import { t } from "../state/i18n"
import { selectedSession, selectSession } from "../state/selection"
import { Chevron } from "./controls"
import { IconArrowUpRight } from "./icons"

/** Marks a subagent that runs in the background; the dashed edge matches its row in the chat. */
export function BackgroundTag() {
  return (
    <span class="shrink-0 rounded border border-dashed border-accent/45 px-1 text-[0.65rem] leading-4 text-accent/80">
      {t("drift.task.background")}
    </span>
  )
}

/** Background workers worth showing: all of them while any is still going or owed to the conversation. */
export function dockTasks(tasks: readonly TaskRecord[] | undefined) {
  const background = (tasks ?? []).filter((task) => task.mode === "background")
  const outstanding = background.some((task) => taskActive(task) || !task.delivered)
  return outstanding ? background : []
}

const stateTone: Record<TaskRecord["state"], string> = {
  queued: "border border-edge-strong",
  running: "bg-accent animate-pulse",
  replied: "bg-ok",
  failed: "bg-danger",
  stopped: "bg-ink-faint",
  interrupted: "bg-warn",
}

export function TaskDock() {
  const engine = useEngine()
  const [open, setOpen] = createSignal(false)
  const tasks = createMemo(() => dockTasks(engine.state.tasks[selectedSession() ?? ""]))
  const finished = () => tasks().filter((task) => !taskActive(task)).length
  const current = () => tasks().find((task) => task.state === "running") ?? tasks().find(taskActive)
  return (
    <Show when={tasks().length > 0}>
      <div class="composer-layer-card dock-card rounded-lg border border-edge bg-surface text-sm">
        <button
          class="flex w-full min-w-0 items-center gap-2 overflow-hidden whitespace-nowrap px-3 py-1.5 text-ink-muted"
          aria-expanded={open()}
          onClick={() => setOpen(!open())}
        >
          <Chevron open={open()} />
          <span class="shrink-0">
            {t("drift.task.title")} · {t("drift.task.progress", { done: finished(), total: tasks().length })}
          </span>
          <Show when={!open()}>
            <span class="min-w-0 flex-1 truncate text-left text-ink-faint">{current()?.description}</span>
          </Show>
        </button>
        <Show when={open()}>
          <ul class="space-y-0.5 border-t border-edge px-2 py-2">
            <For each={tasks()}>{(task) => <TaskRow task={task} />}</For>
          </ul>
        </Show>
      </div>
    </Show>
  )
}

function TaskRow(props: { task: TaskRecord }) {
  const engine = useEngine()
  const activity = () => (props.task.state === "running" ? engine.state.activity[props.task.sessionId]?.current : undefined)
  return (
    <li class="flex items-center gap-2 rounded-md px-2 py-1 text-xs text-ink-muted" title={props.task.delivered ? undefined : (props.task.deliveryError ?? undefined)}>
      <span class={`size-2 shrink-0 rounded-full ${stateTone[props.task.state]}`} />
      <span class="min-w-0 flex-1 truncate">
        <span class="text-ink">{props.task.description}</span>
        <span class="text-ink-faint"> · @{props.task.agent} · {t(`drift.task.state.${props.task.state}`)}</span>
        <Show when={props.task.held && !props.task.delivered}>
          <span class="text-ink-faint"> · {t("drift.task.held")}</span>
        </Show>
        <Show when={!props.task.delivered && props.task.deliveryError}>
          {(reason) => <span class="text-warn"> · {t("drift.task.owed", { reason: reason() })}</span>}
        </Show>
        <Show when={activity()}>{(text) => <span class="font-mono text-accent/80"> · {text()}</span>}</Show>
      </span>
      <Show when={taskActive(props.task)}>
        <button
          class="shrink-0 rounded-md border border-edge px-2 py-0.5 transition-colors hover:border-danger/50 hover:text-danger"
          title={t("drift.task.stopHint")}
          onClick={() => void engine.actions.stopTask(props.task.id)}
        >
          {t("drift.task.stop")}
        </button>
      </Show>
      <button
        class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:bg-overlay hover:text-ink"
        title={t("drift.thread.openSubagent")}
        onClick={() => selectSession(props.task.sessionId)}
      >
        <IconArrowUpRight class="size-3.5" />
      </button>
    </li>
  )
}
