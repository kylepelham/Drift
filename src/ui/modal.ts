export function closeOnBackdropPointerDown(
  event: { target: unknown; currentTarget: unknown },
  onClose: () => void,
) {
  if (event.target === event.currentTarget) onClose()
}
