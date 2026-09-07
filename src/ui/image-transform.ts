export type ImagePoint = { x: number; y: number }
export type ImageSize = { width: number; height: number }
export type ImageTransform = ImagePoint & { scale: number }

// Transform the image, not a giant layout box. 1024x actual pixels stays usable
// without hitting browser scroll-size limits, even for very wide source images.
export const maxImageScale = 1024

export function fitImage(image: ImageSize, viewport: ImageSize): ImageTransform {
  const scale = Math.min(Math.max(1, viewport.width - 32) / image.width, Math.max(1, viewport.height - 32) / image.height, 1)
  return { scale, x: (viewport.width - image.width * scale) / 2, y: (viewport.height - image.height * scale) / 2 }
}

export function zoomImageAt(view: ImageTransform, scale: number, anchor: ImagePoint): ImageTransform {
  const ratio = scale / view.scale
  return { scale, x: anchor.x - (anchor.x - view.x) * ratio, y: anchor.y - (anchor.y - view.y) * ratio }
}

export function containImage(view: ImageTransform, image: ImageSize, viewport: ImageSize): ImageTransform {
  const width = image.width * view.scale
  const height = image.height * view.scale
  // Permit free positioning, but keep enough image on screen to grab it again.
  const marginX = Math.min(64, viewport.width / 2, width / 2)
  const marginY = Math.min(64, viewport.height / 2, height / 2)
  return { ...view, x: Math.min(viewport.width - marginX, Math.max(marginX - width, view.x)), y: Math.min(viewport.height - marginY, Math.max(marginY - height, view.y)) }
}

export function imageWheelScale(delta: number, mode: number, height: number) {
  const pixels = delta * (mode === 1 ? 16 : mode === 2 ? height : 1)
  return Math.exp(-Math.max(-500, Math.min(500, pixels)) * 0.002)
}
