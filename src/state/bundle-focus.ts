import { createSignal } from "solid-js";

/** The bundle Settings > Plugins opens on, when another page sent the user there. */
const [focused, setFocused] = createSignal("");

export const focusBundle = (id: string) => setFocused(id);

/** The bundle asked for, once: reading it clears it. */
export function takeFocusedBundle() {
    const id = focused();
    setFocused("");

    return id;
}
