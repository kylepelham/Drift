import { accent, customTheme, theme } from "./theme";
import { shellInvoke, shellWindow } from "../shell";
import logoSource from "../assets/logo.svg?raw";
import { createEffect } from "solid-js";

const iconSize = 128;
const logo = `data:image/svg+xml,${encodeURIComponent(logoSource)}`;

/** Keeps the desktop window's taskbar icon in the theme's accent, as the logo inside the app is. */
export function bindAppIcon() {
    const invoke = shellInvoke();
    if (!invoke || !shellWindow()) return;

    let shown = "";
    createEffect(() => {
        theme();
        accent();
        customTheme();
        const color = accentColor();
        if (color === shown) return;

        shown = color;
        void paint(color).then((rgba) => invoke("set_window_icon", { rgba, size: iconSize }));
    });
}

function accentColor() {
    const probe = document.createElement("span");
    probe.style.cssText = "display:none;color:var(--accent)";
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
