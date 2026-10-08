import { createSignal, Show } from "solid-js";
import { persisted } from "../state/persist";

/** Logos remembered for installed MCP servers, by name, from the registry they were installed from. */
export const [mcpLogos, setMcpLogos] = persisted<Record<string, string>>("drift.mcp.logos", {}, (value) =>
    value && typeof value === "object" ? (value as Record<string, string>) : {},
);

export function rememberMcpLogo(name: string, image: string | undefined) {
    if (!image) return;
    setMcpLogos({ ...mcpLogos(), [name]: image });
}

export function forgetMcpLogo(name: string) {
    const { [name]: _gone, ...rest } = mcpLogos();
    setMcpLogos(rest);
}

/** A plugin's or server's picture, or its initial on a tile when it has none or it fails to load; cards and rows share it. */
export function LogoTile(props: { image?: string; title: string; large?: boolean }) {
    const [failed, setFailed] = createSignal(false);
    const size = () => (props.large ? "size-12 text-lg" : "size-9 text-sm");
    return (
        <Show
            when={props.image && !failed()}
            fallback={
                <div
                    class={`${size()} flex shrink-0 items-center justify-center rounded-lg bg-raised font-semibold text-ink-muted`}
                    aria-hidden="true"
                >
                    {props.title.slice(0, 1).toUpperCase()}
                </div>
            }
        >
            <img
                src={props.image}
                alt=""
                loading="lazy"
                referrerpolicy="no-referrer"
                class={`${size()} shrink-0 rounded-lg border border-edge/60 bg-raised object-cover`}
                onError={() => setFailed(true)}
            />
        </Show>
    );
}
