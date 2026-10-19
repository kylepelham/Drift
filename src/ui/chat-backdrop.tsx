import { backdropBlur, backdropDim, backdropImage } from "../state/backdrop";
import { Show } from "solid-js";

/** The user's background picture behind the conversation, covered by the theme's background as much as they chose. */
export function ChatBackdrop() {
    return (
        <Show when={backdropImage()}>
            {(image) => (
                <div class="chat-backdrop pointer-events-none absolute inset-0 overflow-hidden" aria-hidden="true">
                    <div
                        class="absolute inset-0 bg-cover bg-center"
                        style={{
                            "background-image": `url("${image()}")`,
                            filter: backdropBlur() ? `blur(${backdropBlur()}px)` : undefined,
                            // Blur pulls the edges in; a little scale keeps them out of view.
                            transform: backdropBlur() ? "scale(1.06)" : undefined,
                        }}
                    />
                    <div class="absolute inset-0 bg-bg" style={{ opacity: backdropDim() }} />
                </div>
            )}
        </Show>
    );
}
