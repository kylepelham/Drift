type ModalEntry = {
  element: HTMLElement
  onClose: () => void
  priority: number
  previous?: HTMLElement
}

export class ModalStack<T> {
  private entries: T[] = []

  constructor(private priority: (entry: T) => number = () => 0) {}

  get size() {
    return this.entries.length
  }

  top() {
    let top: T | undefined
    for (const entry of this.entries) {
      if (top === undefined || this.priority(entry) >= this.priority(top)) top = entry
    }
    return top
  }

  push(entry: T) {
    this.entries.push(entry)
  }

  remove(entry: T) {
    const wasTop = this.top() === entry
    const index = this.entries.indexOf(entry)
    if (index >= 0) this.entries.splice(index, 1)
    return wasTop
  }

  isTop(entry: T) {
    return this.top() === entry
  }
}

const modalStack = new ModalStack<ModalEntry>((entry) => entry.priority)
const inertBeforeModal = new Map<HTMLElement, boolean>()

export function closeOnBackdropPointerDown(
  event: { target: unknown; currentTarget: unknown },
  onClose: () => void,
  modal?: HTMLElement,
) {
  if (event.target !== event.currentTarget) return
  if (modal && !modalIsTopmost(modal)) return
  onClose()
}

export function modalIsTopmost(element: HTMLElement) {
  const top = modalStack.top()
  return !top || top.element === element
}

export function activateModal(element: HTMLElement, onClose: () => void) {
  const overlay = element.closest<HTMLElement>("[data-modal-layer]") ?? element
  const entry: ModalEntry = {
    element,
    onClose,
    priority: Number.parseInt(getComputedStyle(overlay).zIndex, 10) || 0,
    previous: document.activeElement instanceof HTMLElement ? document.activeElement : undefined,
  }
  modalStack.push(entry)
  syncModalInert()

  const onKeyDown = (event: KeyboardEvent) => {
    if (!modalStack.isTop(entry)) return
    if (event.key === "Escape") {
      event.preventDefault()
      event.stopImmediatePropagation()
      entry.onClose()
      return
    }
    if (event.key !== "Tab") return
    const focusable = modalFocusable(element)
    if (!focusable.length) {
      event.preventDefault()
      element.focus()
      return
    }
    const current = focusable.indexOf(document.activeElement as HTMLElement)
    const next = event.shiftKey
      ? current <= 0
        ? focusable.length - 1
        : current - 1
      : current < 0 || current === focusable.length - 1
        ? 0
        : current + 1
    event.preventDefault()
    focusable[next].focus()
  }
  document.addEventListener("keydown", onKeyDown, true)
  queueMicrotask(() => {
    if (!modalStack.isTop(entry)) return
    const target = element.querySelector<HTMLElement>("[autofocus]") ?? modalFocusable(element)[0] ?? element
    target.focus()
  })

  let active = true
  return () => {
    if (!active) return
    active = false
    document.removeEventListener("keydown", onKeyDown, true)
    const wasTop = modalStack.remove(entry)
    syncModalInert()
    if (!wasTop) return
    queueMicrotask(() => {
      const top = modalStack.top()
      if (entry.previous?.isConnected && (!top || top.element.contains(entry.previous))) {
        entry.previous.focus()
        return
      }
      if (top) (modalFocusable(top.element)[0] ?? top.element).focus()
    })
  }
}

function syncModalInert() {
  for (const [element, inert] of inertBeforeModal) element.inert = inert
  const top = modalStack.top()
  if (!top) {
    inertBeforeModal.clear()
    return
  }
  let branch: HTMLElement | null = top.element
  while (branch?.parentElement) {
    for (const sibling of branch.parentElement.children) {
      if (!(sibling instanceof HTMLElement) || sibling === branch) continue
      if (!inertBeforeModal.has(sibling)) inertBeforeModal.set(sibling, sibling.inert)
      sibling.inert = true
    }
    branch = branch.parentElement
    if (branch === document.body) break
  }
}

function modalFocusable(element: HTMLElement) {
  return [...element.querySelectorAll<HTMLElement>(
    'a[href], button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
  )].filter((item) => !item.hidden && item.getClientRects().length > 0)
}
