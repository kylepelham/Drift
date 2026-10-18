import { createSignal, onCleanup } from "solid-js";

export const smallScreenQuery = "(max-width: 719px), (max-height: 500px) and (pointer: coarse)";

export function createMediaQuery(query: string) {
    const media = window.matchMedia(query);
    const [matches, setMatches] = createSignal(media.matches);
    const update = () => setMatches(media.matches);
    media.addEventListener("change", update);
    onCleanup(() => media.removeEventListener("change", update));
    return matches;
}

export function touchInput() {
    return globalThis.matchMedia?.("(pointer: coarse)").matches ?? false;
}
