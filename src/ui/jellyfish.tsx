import { createSignal, onCleanup, onMount, Show } from "solid-js"
import type { Mesh } from "three"
import { DriftLogo } from "./logo"

/** The About mascot loads three.js lazily and disposes every scene resource on cleanup. */
export function Jellyfish(props: { class?: string }) {
  const [fallback, setFallback] = createSignal(true)
  let host!: HTMLDivElement

  onMount(() => {
    if (!canAnimate()) return
    onCleanup(mountScene(() => createScene(host, () => setFallback(false)), () => setFallback(true)))
  })

  return (
    <div class={`relative grid overflow-hidden ${props.class ?? ""}`}>
      <div ref={host} class="absolute inset-0 grid place-items-center" />
      <Show when={fallback()}>
        <div class="relative z-10 grid place-items-center">
          <DriftLogo class="size-16 text-accent" />
        </div>
      </Show>
    </div>
  )
}

/** Reduced motion keeps the static logo: no context, no frame loop, nothing to tear down. */
function canAnimate() {
  return !globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches
}

/** Disposes a scene that finishes loading after its host has unmounted. */
export function mountScene(load: () => Promise<(() => void) | undefined>, onFallback: () => void) {
  let dispose: (() => void) | undefined
  let cancelled = false
  void load()
    .then((created) => {
      if (!created) return cancelled || onFallback()
      if (cancelled) return created()
      dispose = created
    })
    .catch(() => cancelled || onFallback())
  return () => {
    cancelled = true
    dispose?.()
    dispose = undefined
  }
}

/**
 * Resolves the current accent to an `rgb()` string. Reading the raw custom property would hand
 * three.js whatever colour syntax the theme happens to use; a probe returns a computed colour
 * the Color parser always understands.
 */
function accentColor(host: HTMLElement) {
  const probe = document.createElement("span")
  probe.style.cssText = "display:none;color:var(--accent)"
  host.append(probe)
  const value = getComputedStyle(probe).color
  probe.remove()
  return value || "#8fd9fb"
}

async function createScene(host: HTMLElement, ready: () => void) {
  const [THREE, { applyAccent, createJellyfish }] = await Promise.all([
    import("three"),
    import("./jelly/jellyfish"),
  ])
  if (!host.isConnected) return undefined
  const renderer = new THREE.WebGLRenderer({ alpha: true, antialias: true })
  const canvas = renderer.domElement
  if (!renderer.getContext()) {
    renderer.dispose()
    return undefined
  }
  renderer.setPixelRatio(Math.min(globalThis.devicePixelRatio || 1, 2))
  // Explicitly fully transparent: the mascot sits on the About panel, never on its own backdrop.
  renderer.setClearColor(0x000000, 0)
  canvas.style.display = "block"
  canvas.style.background = "transparent"

  const scene = new THREE.Scene()
  const camera = new THREE.PerspectiveCamera(38, 1, 0.1, 40)
  camera.position.set(0, 0, 6.4)
  let jelly: ReturnType<typeof createJellyfish> | undefined

  const pointer = new THREE.Vector2()
  const pointerSmooth = new THREE.Vector2()
  const onPointer = (event: PointerEvent) => {
    const bounds = host.getBoundingClientRect()
    pointer.x = ((event.clientX - bounds.left) / bounds.width) * 2 - 1
    pointer.y = -(((event.clientY - bounds.top) / bounds.height) * 2 - 1)
  }
  const resetPointer = () => pointer.set(0, 0)
  const resize = () => {
    const size = Math.max(1, Math.min(host.clientWidth || 1, host.clientHeight || 1))
    renderer.setSize(size, size, false)
    canvas.style.width = `${size}px`
    canvas.style.height = `${size}px`
    camera.aspect = 1
    camera.updateProjectionMatrix()
  }
  let observer: ResizeObserver | undefined
  // Themes swap documentElement.dataset.theme (and custom themes rewrite the CSS variables), so
  // retint from the same source rather than rebuilding the scene.
  const retint = () => {
    applyAccent(accentColor(host))
    if (!running) renderer.render(scene, camera)
  }
  const themeObserver = new MutationObserver(retint)

  let frame = 0
  let running = false
  const started = performance.now()
  const start = () => {
    if (running || document.hidden) return
    running = true
    frame = requestAnimationFrame(render)
  }
  const stop = () => {
    running = false
    cancelAnimationFrame(frame)
  }
  const visibility = () => (document.hidden ? stop() : start())

  function render(now: number) {
    if (!running || !jelly) return
    const time = (now - started) / 1000
    pointerSmooth.lerp(pointer, 0.08)
    jelly.group.position.y = 0.65 + Math.sin(time * 0.8) * 0.05
    jelly.group.rotation.y = pointerSmooth.x * 0.3 + Math.sin(time * 0.14) * 0.08
    jelly.group.rotation.x = -pointerSmooth.y * 0.12
    jelly.update(time, pointerSmooth)
    renderer.render(scene, camera)
    frame = requestAnimationFrame(render)
  }
  const dispose = () => {
    canvas.remove()
    stop()
    document.removeEventListener("visibilitychange", visibility)
    host.removeEventListener("pointermove", onPointer)
    host.removeEventListener("pointerleave", resetPointer)
    observer?.disconnect()
    themeObserver.disconnect()
    jelly?.group.traverse((object) => {
      const mesh = object as Mesh
      mesh.geometry?.dispose()
      const materials = Array.isArray(mesh.material) ? mesh.material : mesh.material ? [mesh.material] : []
      for (const material of materials) material.dispose()
    })
    scene.clear()
    renderer.dispose()
    renderer.forceContextLoss()
  }

  try {
    // Tint before the first render so the mascot never appears in the stock palette.
    applyAccent(accentColor(host))
    jelly = createJellyfish()
    jelly.group.scale.setScalar(0.72)
    jelly.group.position.y = 0.65
    scene.add(jelly.group)
    resize()
    jelly.update(0, pointerSmooth)
    renderer.render(scene, camera)
    if (!host.isConnected) {
      dispose()
      return undefined
    }
    host.addEventListener("pointermove", onPointer)
    host.addEventListener("pointerleave", resetPointer)
    observer = new ResizeObserver(resize)
    observer.observe(host)
    themeObserver.observe(document.documentElement, { attributeFilter: ["data-theme", "style", "class"] })
    document.addEventListener("visibilitychange", visibility)
    host.append(canvas)
    ready()
    start()
    return dispose
  } catch (error) {
    dispose()
    throw error
  }
}
