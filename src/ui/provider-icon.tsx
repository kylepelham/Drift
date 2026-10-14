/** Ollama on this machine and Ollama Cloud share one mark; the sprite holds it under the cloud's id. */
const shared: Record<string, string> = { ollama: "ollama-cloud" };

export function ProviderIcon(props: { id?: string; class?: string }) {
    const symbol = () => shared[props.id ?? ""] ?? props.id ?? "opencode";

    return (
        <svg class={props.class ?? "size-4"} viewBox="0 0 40 40" fill="currentColor">
            <use href={`/provider-icons.svg#${symbol()}`} />
        </svg>
    );
}
