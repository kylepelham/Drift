export function ProviderIcon(props: { id?: string; class?: string }) {
  return (
    <svg class={props.class ?? "size-4"} viewBox="0 0 40 40" fill="currentColor">
      <use href={`/provider-icons.svg#${props.id ?? "opencode"}`} />
    </svg>
  )
}
