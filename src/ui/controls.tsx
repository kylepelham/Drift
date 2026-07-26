/**
 * Small presentational controls shared across unrelated feature modules.
 *
 * These previously lived inside whichever feature happened to need them first (Chevron in the tool
 * output renderer, Toggle in the model manager), which made the import graph imply relationships
 * that do not exist.
 */

/** Disclosure arrow that rotates to point down when its section is open. */
export function Chevron(props: { open: boolean }) {
  return (
    <svg
      class="size-3 shrink-0 transition-transform duration-150"
      classList={{ "rotate-90": props.open }}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
    >
      <path d="M6 4l4 4-4 4" />
    </svg>
  )
}

/**
 * Switch-style toggle. Pointer and click events are stopped so the toggle stays usable inside
 * draggable or clickable rows without triggering the row's own handler.
 */
export function Toggle(props: { label: string; checked: boolean; disabled?: boolean; onChange: () => void }) {
  return (
    <button
      role="switch"
      aria-label={props.label}
      aria-checked={props.checked}
      disabled={props.disabled}
      class="shrink-0 disabled:opacity-50"
      onPointerDown={(event) => event.stopPropagation()}
      onClick={(event) => {
        event.stopPropagation()
        props.onChange()
      }}
    >
      <ToggleTrack checked={props.checked} />
    </button>
  )
}

/**
 * The visual track and knob on their own, for rows that already act as the switch and only need
 * the indicator rather than a nested button.
 */
export function ToggleTrack(props: { checked: boolean }) {
  return (
    <span
      class="relative block h-4 w-7 shrink-0 rounded-full border transition-colors"
      classList={{
        "border-accent bg-accent": props.checked,
        "border-edge-strong bg-raised": !props.checked,
      }}
    >
      <span
        class="absolute top-0.5 left-0.5 size-2.5 rounded-full transition-[transform,background-color]"
        classList={{
          "translate-x-3 bg-accent-ink": props.checked,
          "bg-ink-muted": !props.checked,
        }}
      />
    </span>
  )
}
