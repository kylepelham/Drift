import { createSignal, onCleanup, onMount, Show } from "solid-js"
import { DriftLogo } from "./logo"

/**
 * The About page mascot: a procedural jellyfish, the same idea as the one on driftagent.dev.
 *
 * three.js is imported only when this mounts, so nothing WebGL related is in the startup bundle.
 * Everything the scene allocates is released again in `dispose`: the frame loop is cancelled, the
 * GPU resources are disposed, the context is force-lost, and the canvas leaves the DOM. Closing
 * settings therefore leaves no renderer, no context, and no running animation behind.
 */
export function Jellyfish(props: { class?: string }) {
  const [fallback, setFallback] = createSignal(!canAnimate())
  let host!: HTMLDivElement

  onMount(() => {
    if (!canAnimate()) return
    let dispose: (() => void) | undefined
    let cancelled = false
    void createScene(host)
      .then((created) => {
        if (cancelled || !created) return setFallback(true)
        dispose = created
      })
      .catch(() => setFallback(true))
    onCleanup(() => {
      cancelled = true
      dispose?.()
    })
  })

  return (
    <div ref={host} class={`relative grid place-items-center ${props.class ?? ""}`}>
      <Show when={fallback()}>
        <DriftLogo class="size-16 text-accent" />
      </Show>
    </div>
  )
}

/** Reduced motion keeps the static logo: no context, no frame loop, nothing to tear down. */
function canAnimate() {
  return !globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches
}

const bellColor = 0x8fd9fb
const tentacleCount = 8
const tentacleSegments = 14

async function createScene(host: HTMLElement) {
  const THREE = await import("three")
  const renderer = new THREE.WebGLRenderer({ alpha: true, antialias: true })
  const canvas = renderer.domElement
  if (!renderer.getContext()) {
    renderer.dispose()
    return undefined
  }
  renderer.setPixelRatio(Math.min(globalThis.devicePixelRatio || 1, 2))
  canvas.style.display = "block"
  host.append(canvas)

  const scene = new THREE.Scene()
  const camera = new THREE.PerspectiveCamera(38, 1, 0.1, 100)
  camera.position.set(0, 0.15, 5.2)
  scene.add(new THREE.AmbientLight(0xffffff, 1.1))
  const key = new THREE.DirectionalLight(0xffffff, 1.4)
  key.position.set(1.4, 2.2, 2.6)
  scene.add(key)

  const jelly = new THREE.Group()
  scene.add(jelly)

  const bellGeometry = new THREE.SphereGeometry(1, 48, 32, 0, Math.PI * 2, 0, Math.PI * 0.58)
  const bellMaterial = new THREE.MeshStandardMaterial({
    color: bellColor,
    emissive: bellColor,
    emissiveIntensity: 0.28,
    transparent: true,
    opacity: 0.62,
    roughness: 0.28,
    metalness: 0.05,
    side: THREE.DoubleSide,
  })
  const bell = new THREE.Mesh(bellGeometry, bellMaterial)
  jelly.add(bell)

  const rimGeometry = new THREE.TorusGeometry(0.94, 0.035, 12, 64)
  const rimMaterial = new THREE.MeshStandardMaterial({
    color: bellColor,
    emissive: bellColor,
    emissiveIntensity: 0.6,
    transparent: true,
    opacity: 0.85,
    roughness: 0.35,
  })
  const rim = new THREE.Mesh(rimGeometry, rimMaterial)
  rim.rotation.x = Math.PI / 2
  rim.position.y = 0.02
  jelly.add(rim)

  const tentacleMaterial = new THREE.LineBasicMaterial({ color: bellColor, transparent: true, opacity: 0.5 })
  const tentacles = Array.from({ length: tentacleCount }, (_, index) => {
    const geometry = new THREE.BufferGeometry()
    const position = new THREE.BufferAttribute(new Float32Array(tentacleSegments * 3), 3)
    geometry.setAttribute("position", position)
    jelly.add(new THREE.Line(geometry, tentacleMaterial))
    return { geometry, position, angle: (index / tentacleCount) * Math.PI * 2 }
  })

  const resize = () => {
    const size = Math.max(1, Math.min(host.clientWidth || 1, host.clientHeight || 1))
    renderer.setSize(size, size, false)
    canvas.style.width = `${size}px`
    canvas.style.height = `${size}px`
    camera.aspect = 1
    camera.updateProjectionMatrix()
  }
  resize()
  const observer = new ResizeObserver(resize)
  observer.observe(host)

  let frame = 0
  let running = false
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
  document.addEventListener("visibilitychange", visibility)

  function render(now: number) {
    if (!running) return
    const time = now / 1000
    const pulse = Math.sin(time * 1.6)
    bell.scale.set(1 + pulse * 0.08, 1 - pulse * 0.13, 1 + pulse * 0.08)
    rim.scale.setScalar(1 + pulse * 0.08)
    rim.position.y = 0.02 - pulse * 0.1
    jelly.position.y = Math.sin(time * 0.8) * 0.12
    jelly.rotation.y = time * 0.35

    for (const tentacle of tentacles) {
      for (let segment = 0; segment < tentacleSegments; segment++) {
        const drop = segment / (tentacleSegments - 1)
        const sway = Math.sin(time * 2.1 - drop * 3.4 + tentacle.angle) * 0.16 * drop
        const spread = 0.72 + drop * 0.14 + sway
        tentacle.position.setXYZ(
          segment,
          Math.cos(tentacle.angle) * spread,
          -0.05 - drop * 1.85 - pulse * 0.12 * drop,
          Math.sin(tentacle.angle) * spread,
        )
      }
      tentacle.position.needsUpdate = true
      tentacle.geometry.computeBoundingSphere()
    }

    renderer.render(scene, camera)
    frame = requestAnimationFrame(render)
  }
  start()

  return () => {
    stop()
    document.removeEventListener("visibilitychange", visibility)
    observer.disconnect()
    for (const tentacle of tentacles) tentacle.geometry.dispose()
    tentacleMaterial.dispose()
    bellGeometry.dispose()
    bellMaterial.dispose()
    rimGeometry.dispose()
    rimMaterial.dispose()
    scene.clear()
    renderer.dispose()
    renderer.forceContextLoss()
    canvas.remove()
  }
}
