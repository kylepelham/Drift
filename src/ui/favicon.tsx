import { createResource, createSignal, Show } from "solid-js";
import { useEngine } from "../engine";

/** Icons by host, loaded once a session of the app; a host without one is remembered as such. */
const icons = new Map<string, Promise<string | undefined>>();

/** A site's small icon, beside a URL the agent fetched; nothing shows until it loads, or if it never does. */
export function Favicon(props: { host: string }) {
    const engine = useEngine();
    const [failed, setFailed] = createSignal(false);
    const [url] = createResource(
        () => props.host,
        (host) => {
            const known = icons.get(host);
            if (known) return known;
            const loading = engine.actions
                .favicon(host)
                .then((icon) => `data:${icon.mime};base64,${icon.data}`)
                .catch(() => undefined);
            icons.set(host, loading);
            return loading;
        },
    );

    return (
        <Show when={!failed() && url()}>
            {(src) => (
                <img
                    src={src()}
                    alt=""
                    aria-hidden="true"
                    class="tool-favicon size-3.5 shrink-0 rounded-sm object-contain"
                    onError={() => setFailed(true)}
                />
            )}
        </Show>
    );
}
