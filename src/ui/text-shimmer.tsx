import { createEffect, createSignal, onCleanup } from "solid-js"

export function TextShimmer(props: { text: string; active?: boolean; class?: string }) {
  const active = () => props.active ?? true
  const [running, setRunning] = createSignal(active())
  let timer: ReturnType<typeof setTimeout> | undefined

  createEffect(() => {
    if (timer) clearTimeout(timer)
    if (active()) {
      setRunning(true)
      return
    }
    timer = setTimeout(() => setRunning(false), 220)
  })
  onCleanup(() => timer && clearTimeout(timer))

  return (
    <span class={props.class} data-component="text-shimmer" data-active={active() ? "true" : "false"} aria-label={props.text}>
      <span data-slot="text-shimmer-char">
        <span data-slot="text-shimmer-base" aria-hidden="true">
          {props.text}
        </span>
        <span data-slot="text-shimmer-sweep" data-run={running() ? "true" : "false"} aria-hidden="true">
          {props.text}
        </span>
      </span>
    </span>
  )
}
