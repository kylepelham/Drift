import { persisted } from "./persist";

/** The longest side a background image is kept at: sharp on a large screen, small enough for local storage. */
const maxSide = 2400;
const jpegQuality = 0.86;
const imageKey = "drift.backdrop.image";

/**
 * A picture behind the conversation, with how much the theme covers it and how blurred it is. It stays on this
 * device: an image is too large to mirror to the shell or a phone.
 */
const [backdropImage, setBackdropImageValue] = persisted<string>(imageKey, "");
export { backdropImage };
export const [backdropDim, setBackdropDim] = persisted<number>("drift.backdrop.dim", 0.7);
export const [backdropBlur, setBackdropBlur] = persisted<number>("drift.backdrop.blur", 0);

/** Keeps `image` as the background; false when local storage has no room for it, and nothing changes. */
export function setBackdropImage(image: string) {
    try {
        if (image) localStorage.setItem(imageKey, JSON.stringify(image));
        else localStorage.removeItem(imageKey);
    } catch {
        return false;
    }

    setBackdropImageValue(image);
    return true;
}

/** An image file as a JPEG data URL no larger than `maxSide` on its longest side. */
export async function backdropFromFile(file: File): Promise<string> {
    const bitmap = await createImageBitmap(file);
    const scale = Math.min(1, maxSide / Math.max(bitmap.width, bitmap.height));
    const canvas = document.createElement("canvas");
    canvas.width = Math.round(bitmap.width * scale);
    canvas.height = Math.round(bitmap.height * scale);

    canvas.getContext("2d")?.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    bitmap.close();
    return canvas.toDataURL("image/jpeg", jpegQuality);
}
