import type { JSX } from "solid-js"

/**
 * Layout primitives shared by the settings sections. They live here rather than in settings.tsx so a
 * section can be split into its own file without importing back into the module that renders it.
 */

/** A titled block of rows. */
export function SettingsGroup(props: { title: string; children: JSX.Element }) {
  return (
    <section>
      <div class="mb-1.5 text-[0.68rem] font-semibold tracking-wide text-ink-faint uppercase">{props.title}</div>
      <div class="border-y border-edge/80">{props.children}</div>
    </section>
  )
}

/** A label/description pair on the left with its control on the right. */
export function SettingsRow(props: {
  title: string
  description: string
  children: JSX.Element
  disabled?: boolean
  onClick?: () => void
}) {
  return (
    <div
      class="flex min-h-13 flex-col items-stretch gap-2 border-b border-edge/70 px-1 py-2.5 last:border-b-0 sm:flex-row sm:items-center sm:gap-4"
      classList={{
        "cursor-pointer hover:bg-raised/40": !!props.onClick && !props.disabled,
        "opacity-50": !!props.disabled,
      }}
      onClick={() => !props.disabled && props.onClick?.()}
    >
      <div class="min-w-0 flex-1">
        <div class="text-[0.82rem] font-medium text-ink">{props.title}</div>
        <div class="mt-0.5 text-[0.72rem] leading-relaxed text-ink-faint">{props.description}</div>
      </div>
      <div class="shrink-0 self-end sm:self-auto">{props.children}</div>
    </div>
  )
}
