import type { JSX } from "solid-js"
import logoSource from "../assets/logo.svg?raw"

// Inlined rather than referenced by URL: `.drift-logo` paints `background: currentColor` behind
// the mask, so a mask image that is still loading shows the element's full square in the accent
// colour for a frame. A data URI is available synchronously, so the mark never flashes as a block.
const logo = `data:image/svg+xml,${encodeURIComponent(logoSource)}`

export function DriftLogo(props: { class?: string; label?: string }) {
  const style: JSX.CSSProperties = {
    "--drift-logo-image": `url("${logo}")`,
  }
  return (
    <span
      class={`drift-logo ${props.class ?? ""}`}
      style={style}
      role={props.label ? "img" : undefined}
      aria-label={props.label}
      aria-hidden={props.label ? undefined : "true"}
    />
  )
}
