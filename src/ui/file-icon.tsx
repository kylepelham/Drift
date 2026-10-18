import { fileIconName } from "./file-icons";

// Vite replaces the call, never a check for the function, so try it; Bun's test runner throws.
function iconUrls(): Record<string, string> {
    try {
        return import.meta.glob<string>("../assets/file-icons/*.svg", {
            eager: true,
            query: "?url",
            import: "default",
        });
    } catch {
        return {};
    }
}
const urls = iconUrls();

/** A file's language icon, the way an editor shows it. */
export function FileIcon(props: { path: string; class?: string }) {
    const src = () => urls[`../assets/file-icons/${fileIconName(props.path)}.svg`];

    return (
        <img src={src()} alt="" aria-hidden="true" draggable={false} class={`shrink-0 ${props.class ?? "size-4"}`} />
    );
}
