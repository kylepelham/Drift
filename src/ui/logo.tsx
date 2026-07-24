import type { JSX } from "solid-js"
import logo from "../assets/logo.svg"

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
