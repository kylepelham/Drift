import { shellEvents, shellInvoke, shellWindow } from "../shell";
import { createEffect, createSignal } from "solid-js";
import { accent, customTheme, theme } from "./theme";
import logoSource from "../assets/logo.svg?raw";

const iconSize = 128;
const logo = `data:image/svg+xml,${encodeURIComponent(logoSource)}`;

/** Light or dark as the taskbar is; null where there is none to read, undefined until the shell answers. */
const [taskbarLight, setTaskbarLight] = createSignal<boolean | null>();

/** Keeps the desktop window's taskbar icon in the theme's accent, in the shade that stands out on the taskbar. */
export function bindAppIcon() {
    const invoke = shellInvoke();
    if (!invoke || !shellWindow()) return;

    void invoke<boolean | null>("taskbar_light").then(setTaskbarLight, () => setTaskbarLight(null));
    void shellEvents()?.listen<boolean | null>("taskbar-theme-changed", (event) => setTaskbarLight(event.payload));

    let shown = "";
    createEffect(() => {
        theme();
        accent();
        customTheme();
        const light = taskbarLight();
        if (light === undefined) return;

        const color = iconColor(light);
        if (color === shown) return;

        shown = color;
        void paint(color).then((rgba) => invoke("set_window_icon", { rgba, size: iconSize }));
    });
}

/** The accent's shade for the taskbar's tone; an accent set by custom CSS, or with no taskbar tone, the theme's. */
function iconColor(light: boolean | null) {
    const current = cssColor("var(--accent)");
    if (light === null) return current;

    const onDark = cssColor("var(--accent-d)");
    const onLight = cssColor("var(--accent-l)");
    if (current !== onDark && current !== onLight) return current;

    return light ? onLight : onDark;
}

function cssColor(value: string) {
    const probe = document.createElement("span");
    probe.style.cssText = `display:none;color:${value}`;
    document.body.append(probe);
    const color = getComputedStyle(probe).color;
    probe.remove();
    return color;
}

async function paint(color: string) {
    const image = new Image();
    image.src = logo;
    await image.decode();

    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = iconSize;
    const context = canvas.getContext("2d")!;
    context.fillStyle = color;
    context.fillRect(0, 0, iconSize, iconSize);
    context.globalCompositeOperation = "destination-in";
    context.drawImage(image, 0, 0, iconSize, iconSize);

    return Array.from(context.getImageData(0, 0, iconSize, iconSize).data);
}
