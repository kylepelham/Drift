/** Routes that share a mark with another: the sprite holds it once, under that route's id. */
const shared: Record<string, string> = {
    ollama: "ollama-cloud",
    "kimi-code-plan-global": "kimi-for-coding",
    "kimi-code-plan-cn": "kimi-for-coding",
    "alibaba-coding-plan": "alibaba",
    "alibaba-coding-plan-cn": "alibaba-cn",
};

export function ProviderIcon(props: { id?: string; class?: string }) {
    const symbol = () => shared[props.id ?? ""] ?? props.id ?? "opencode";

    return (
        <svg class={props.class ?? "size-4"} viewBox="0 0 40 40" fill="currentColor">
            <use href={`/provider-icons.svg#${symbol()}`} />
        </svg>
    );
}
