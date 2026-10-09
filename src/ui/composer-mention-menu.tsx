import { For } from "solid-js";

import type { createMentionAutocomplete } from "./composer-mentions";

/** The `@` file suggestions above the composer. */
export function ComposerMentionMenu(props: { mention: ReturnType<typeof createMentionAutocomplete> }) {
    return (
        <div class="pop-in absolute bottom-full left-3 z-20 mb-2 w-96 overflow-hidden rounded-lg border border-edge bg-overlay py-1 shadow-xl shadow-black/30">
            <For each={props.mention.hits()}>
                {(path, index) => (
                    <button
                        class="flex w-full items-center px-3 py-1.5 text-left font-mono text-xs transition-colors"
                        classList={{
                            "bg-raised text-ink": index() === props.mention.activeIndex(),
                            "text-ink-muted": index() !== props.mention.activeIndex(),
                        }}
                        onMouseEnter={() => props.mention.setCursor(index())}
                        onClick={() => props.mention.pick(path)}
                    >
                        <span class="truncate">{path}</span>
                    </button>
                )}
            </For>
        </div>
    );
}
