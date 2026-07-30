import * as THREE from "three"
import {
  bellVertex,
  bellFragment,
  tentacleVertex,
  tentacleFragment,
  glowVertex,
  glowFragment,
  faceVertex,
  blushFragment,
} from "./shaders"

const AQUA = new THREE.Color("#8fd9fb")
const AQUA_LIGHT = new THREE.Color("#d4f2ff")
const AQUA_DEEP = new THREE.Color("#4f93cc")
const RIM = new THREE.Color("#b8ecff")
const INK = new THREE.Color("#0f1626")
const BLUSH = new THREE.Color("#ffa9b8")
const FACE_RADIUS = 1.06

export interface Jellyfish {
  group: THREE.Group
  update(time: number, pointer: THREE.Vector2): void
}

interface TimeUniforms {
  uTime: { value: number }
  uPulse: { value: number }
}

function bellMaterial(side: THREE.Side, depthWrite: boolean): THREE.ShaderMaterial {
  return new THREE.ShaderMaterial({
    vertexShader: bellVertex,
    fragmentShader: bellFragment,
    uniforms: {
      uTime: { value: 0 },
      uPulse: { value: 0 },
      uTop: { value: AQUA_LIGHT },
      uBottom: { value: AQUA },
      uRim: { value: RIM },
    },
    transparent: true,
    depthWrite,
    side,
  })
}

function makeBell(): THREE.Mesh[] {
  const geo = new THREE.SphereGeometry(1, 128, 96, 0, Math.PI * 2, 0, Math.PI * 0.62)
  geo.translate(0, 0.12, 0)
  const back = new THREE.Mesh(geo, bellMaterial(THREE.BackSide, false))
  const front = new THREE.Mesh(geo, bellMaterial(THREE.FrontSide, false))
  back.renderOrder = 4
  front.renderOrder = 7
  return [back, front]
}

function makeFrill(): THREE.Mesh {
  const geo = new THREE.SphereGeometry(0.82, 96, 48, 0, Math.PI * 2, Math.PI * 0.3, Math.PI * 0.4)
  geo.translate(0, 0.06, 0)
  const mat = bellMaterial(THREE.DoubleSide, false)
  mat.uniforms.uTop!.value = AQUA
  mat.uniforms.uBottom!.value = AQUA_DEEP
  const mesh = new THREE.Mesh(geo, mat)
  mesh.renderOrder = 2
  return mesh
}

function tentacleMaterial(phase: number, amp: number, len: number): THREE.ShaderMaterial {
  return new THREE.ShaderMaterial({
    vertexShader: tentacleVertex,
    fragmentShader: tentacleFragment,
    uniforms: {
      uTime: { value: 0 },
      uPulse: { value: 0 },
      uPhase: { value: phase },
      uAmp: { value: amp },
      uLen: { value: len },
      uColor: { value: AQUA },
      uTip: { value: AQUA_DEEP },
      uFade: { value: 0.85 },
    },
    transparent: true,
    depthWrite: false,
    side: THREE.DoubleSide,
  })
}

function makeTentacle(radius: number, len: number, angle: number, dist: number, phase: number, amp: number): THREE.Mesh {
  const geo = new THREE.CylinderGeometry(radius, radius * 0.28, len, 10, 72, true)
  geo.translate(0, -len / 2, 0)
  const mesh = new THREE.Mesh(geo, tentacleMaterial(phase, amp, len))
  mesh.position.set(Math.cos(angle) * dist, -0.15, Math.sin(angle) * dist)
  mesh.renderOrder = 3
  return mesh
}

function makeTentacles(): THREE.Mesh[] {
  const thick = [0, 1, 2].map((i) => {
    const angle = (i / 3) * Math.PI * 2 + 0.5
    return makeTentacle(0.085, 1.9, angle, 0.42, i * 2.1, 0.24)
  })
  const thin = [0, 1, 2, 3, 4].map((i) => {
    const angle = (i / 5) * Math.PI * 2
    return makeTentacle(0.02, 2.6, angle, 0.6, i * 1.3 + 0.7, 0.34)
  })
  return [...thick, ...thin]
}

function makeCore(): THREE.Mesh {
  const geo = new THREE.SphereGeometry(0.5, 48, 32)
  geo.translate(0, 0.28, 0)
  const mat = new THREE.ShaderMaterial({
    vertexShader: glowVertex,
    fragmentShader: glowFragment,
    uniforms: { uPulse: { value: 0 }, uColor: { value: AQUA_LIGHT } },
    transparent: true,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
  })
  const mesh = new THREE.Mesh(geo, mat)
  mesh.renderOrder = 1
  return mesh
}

function flatCircle(radius: number, color: THREE.Color, blush: boolean): THREE.Mesh {
  const geo = new THREE.CircleGeometry(radius, 48)
  const mat = blush
    ? new THREE.ShaderMaterial({
        vertexShader: faceVertex,
        fragmentShader: blushFragment,
        uniforms: { uColor: { value: color } },
        transparent: true,
        depthWrite: false,
        depthTest: false,
      })
    : new THREE.MeshBasicMaterial({ color, transparent: true, depthWrite: false, depthTest: false })
  return new THREE.Mesh(geo, mat)
}

function bellPoint(x: number, y: number): THREE.Vector3 {
  const z2 = FACE_RADIUS * FACE_RADIUS - x * x - y * y
  return new THREE.Vector3(x, y, Math.sqrt(Math.max(0.01, z2)))
}

function surfaceHolder(x: number, y: number, child: THREE.Object3D): THREE.Group {
  const holder = new THREE.Group()
  const pos = bellPoint(x, y)
  holder.position.copy(pos)
  holder.quaternion.setFromUnitVectors(new THREE.Vector3(0, 0, 1), pos.clone().normalize())
  holder.add(child)
  return holder
}

function makeEye(x: number): THREE.Group {
  const eye = new THREE.Group()
  const pupil = flatCircle(0.085, INK, false)
  const shine = flatCircle(0.026, new THREE.Color("#ffffff"), false)
  shine.name = "shine"
  shine.position.set(0.028, 0.032, 0.01)
  eye.add(pupil, shine)
  return surfaceHolder(x, 0.28, eye)
}

function makeMouth(): THREE.Group {
  const geo = new THREE.TorusGeometry(0.055, 0.011, 8, 32, Math.PI * 0.9)
  const mat = new THREE.MeshBasicMaterial({ color: INK, transparent: true, depthWrite: false, depthTest: false })
  const mesh = new THREE.Mesh(geo, mat)
  mesh.rotation.z = Math.PI + Math.PI * 0.05
  return surfaceHolder(0, 0.13, mesh)
}

function makeFace(): THREE.Group {
  const face = new THREE.Group()
  const left = makeEye(-0.22)
  const right = makeEye(0.22)
  const blushL = surfaceHolder(-0.38, 0.14, flatCircle(0.09, BLUSH, true))
  const blushR = surfaceHolder(0.38, 0.14, flatCircle(0.09, BLUSH, true))
  face.add(left, right, makeMouth(), blushL, blushR)
  face.traverse((obj) => {
    obj.renderOrder = obj.name === "shine" ? 6 : 5
  })
  face.userData = { left, right }
  return face
}

function pulseWave(time: number): number {
  const primary = Math.sin(time * 1.7)
  const echo = Math.sin(time * 3.4 + 1.1) * 0.3
  return (primary + echo) / 1.3
}

function blinkScale(time: number): number {
  const cycle = time % 4.6
  if (cycle < 0.14) return Math.abs(Math.sin((cycle / 0.14) * Math.PI)) > 0.5 ? 0.08 : 1
  return 1
}

export function createJellyfish(): Jellyfish {
  const group = new THREE.Group()
  const [bellBack, bellFront] = makeBell()
  const frill = makeFrill()
  const tentacles = makeTentacles()
  const core = makeCore()
  const face = makeFace()
  group.add(core, frill, ...tentacles, bellBack!, bellFront!, face)

  const timed: TimeUniforms[] = [bellBack!, bellFront!, frill, ...tentacles].map(
    (mesh) => (mesh.material as THREE.ShaderMaterial).uniforms as unknown as TimeUniforms,
  )
  const coreUniforms = (core.material as THREE.ShaderMaterial).uniforms

  function update(time: number, pointer: THREE.Vector2) {
    const pulse = pulseWave(time)
    for (const u of timed) {
      u.uTime.value = time
      u.uPulse.value = pulse
    }
    coreUniforms.uPulse!.value = pulse * 0.5 + 0.5

    const blink = blinkScale(time)
    const { left, right } = face.userData as { left: THREE.Group; right: THREE.Group }
    const lookX = THREE.MathUtils.clamp(pointer.x * 0.09, -0.09, 0.09)
    const lookY = THREE.MathUtils.clamp(pointer.y * 0.07, -0.07, 0.07)
    for (const eye of [left, right]) {
      const baseX = eye === left ? -0.22 : 0.22
      eye.scale.y = blink
      eye.position.copy(bellPoint(baseX + lookX, 0.28 + lookY))
    }
    face.rotation.y = pointer.x * 0.16
    face.rotation.x = -pointer.y * 0.1
  }

  return { group, update }
}
