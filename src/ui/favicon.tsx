import { createResource, createSignal, Show } from "solid-js";
import { useEngine } from "../engine";

/** Icons by host while they load or once they have; a host without one is asked again on a later mount. */
const icons = new Map<string, Promise<string | undefined>>();

/** Below this contrast against the transcript's background an icon gets a backing in the opposite tone. */
const minContrast = 2;

/** A site's small icon, beside a URL the agent fetched; nothing shows until it loads, or if it never does. */
export function Favicon(props: { host: string }) {
    const engine = useEngine();
    const [failed, setFailed] = createSignal(false);
    const [backing, setBacking] = createSignal<string>();
    const [url] = createResource(
        () => props.host,
        (host) => {
            const known = icons.get(host);
            if (known) return known;
            const loading = engine.actions
                .favicon(host)
                .then((icon) => `data:${icon.mime};base64,${icon.data}`)
                .catch(() => {
                    icons.delete(host);
                    return undefined;
                });
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
                    style={backing() ? { background: backing(), padding: "1px" } : undefined}
                    onLoad={(event) => setBacking(iconBacking(event.currentTarget))}
                    onError={() => setFailed(true)}
                />
            )}
        </Show>
    );
}

/** A backing colour for an icon too close in tone to the background it sits on: GitHub's black mark on a dark theme. */
function iconBacking(image: HTMLImageElement) {
    const icon = iconLuminance(image);
    const background = colorLuminance(getComputedStyle(image).getPropertyValue("--bg"));
    if (icon === undefined || background === undefined) return undefined;

    const contrast = (Math.max(icon, background) + 0.05) / (Math.min(icon, background) + 0.05);
    if (contrast >= minContrast) return undefined;

    return background < 0.5 ? "rgb(255 255 255 / 0.9)" : "rgb(0 0 0 / 0.75)";
}

const sample = 16;

/** The mean luminance of an icon's visible pixels; nothing for one that fills its square, as it has its own backing. */
function iconLuminance(image: HTMLImageElement) {
    const context = canvasContext();
    if (!context) return undefined;
    context.clearRect(0, 0, sample, sample);
    context.drawImage(image, 0, 0, sample, sample);
    const pixels = context.getImageData(0, 0, sample, sample).data;
    let total = 0;
    let visible = 0;
    for (let at = 0; at < pixels.length; at += 4) {
        if (pixels[at + 3] < 128) continue;
        total += luminance(pixels[at], pixels[at + 1], pixels[at + 2]);
        visible++;
    }
    const filled = visible / (sample * sample) > 0.9;
    return visible && !filled ? total / visible : undefined;
}

function colorLuminance(color: string) {
    const context = canvasContext();
    if (!context || !color.trim()) return undefined;
    context.clearRect(0, 0, 1, 1);
    context.fillStyle = color.trim();
    context.fillRect(0, 0, 1, 1);
    const [red, green, blue] = context.getImageData(0, 0, 1, 1).data;
    return luminance(red, green, blue);
}

let canvas: CanvasRenderingContext2D | null | undefined;

function canvasContext() {
    if (canvas === undefined) {
        const element = document.createElement("canvas");
        element.width = sample;
        element.height = sample;
        canvas = element.getContext("2d", { willReadFrequently: true });
    }
    return canvas;
}

/** WCAG relative luminance of an sRGB colour. */
function luminance(red: number, green: number, blue: number) {
    const linear = (channel: number) => {
        const value = channel / 255;
        return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue);
}
