import { fileIconName } from "./file-icons";

// Bun's test runner has no import.meta.glob; icons simply stay blank there.
const urls =
    typeof import.meta.glob === "function"
        ? import.meta.glob<string>("../assets/file-icons/*.svg", { eager: true, query: "?url", import: "default" })
        : {};

/** A file's language icon, the way an editor shows it. */
export function FileIcon(props: { path: string; class?: string }) {
    const src = () => urls[`../assets/file-icons/${fileIconName(props.path)}.svg`];

    return (
        <img src={src()} alt="" aria-hidden="true" draggable={false} class={`shrink-0 ${props.class ?? "size-4"}`} />
    );
}
