export const bellVertex = /* glsl */ `
uniform float uTime;
uniform float uPulse;
varying vec3 vNormal;
varying vec3 vView;
varying float vSkirt;
varying float vY;

void main() {
  vec3 pos = position;
  float y01 = clamp((pos.y + 0.35) / 1.35, 0.0, 1.0);
  float skirt = pow(1.0 - y01, 1.6);
  float ang = atan(pos.z, pos.x);

  float breathe = uPulse * (0.09 * skirt - 0.035 * (1.0 - skirt));
  pos.xz *= 1.0 + breathe;
  pos.y *= 1.0 - uPulse * 0.045 * (1.0 - skirt);

  float scallop = sin(ang * 9.0 + uTime * 0.9) * 0.045 * skirt;
  float ripple = sin(ang * 4.0 - uTime * 1.4) * 0.02 * skirt;
  pos.xz *= 1.0 + scallop + ripple;
  pos.x += sin(uTime * 0.7) * 0.03 * skirt;
  pos.z += cos(uTime * 0.55) * 0.02 * skirt;

  vSkirt = skirt;
  vY = y01;
  vNormal = normalize(normalMatrix * normal);
  vec4 mv = modelViewMatrix * vec4(pos, 1.0);
  vView = normalize(-mv.xyz);
  gl_Position = projectionMatrix * mv;
}
`

export const bellFragment = /* glsl */ `
uniform vec3 uTop;
uniform vec3 uBottom;
uniform vec3 uRim;
uniform float uPulse;
varying vec3 vNormal;
varying vec3 vView;
varying float vSkirt;
varying float vY;

void main() {
  vec3 n = gl_FrontFacing ? vNormal : -vNormal;
  float fres = pow(1.0 - max(dot(n, vView), 0.0), 2.4);
  vec3 base = mix(uBottom, uTop, smoothstep(0.15, 0.95, vY));
  vec3 col = mix(base, uRim, fres * 0.85);
  col += uRim * uPulse * 0.08;
  float inner = gl_FrontFacing ? 0.0 : 0.35;
  col *= 1.0 - inner;
  float alpha = 0.4 + fres * 0.38 + vSkirt * 0.08;
  gl_FragColor = vec4(col, alpha);
}
`

export const tentacleVertex = /* glsl */ `
uniform float uTime;
uniform float uPulse;
uniform float uPhase;
uniform float uAmp;
uniform float uLen;
varying float vT;
varying vec2 vUv;

void main() {
  vec3 pos = position;
  float t = clamp(-pos.y / uLen, 0.0, 1.0);
  float sway = t * t;
  pos.x += sin(uTime * 1.9 - t * 4.5 + uPhase) * uAmp * sway;
  pos.z += cos(uTime * 1.4 - t * 3.2 + uPhase * 1.7) * uAmp * 0.7 * sway;
  pos.x += sin(uTime * 0.6 + uPhase) * 0.06 * sway;
  pos.y -= uPulse * 0.1 * t;
  vT = t;
  vUv = uv;
  gl_Position = projectionMatrix * modelViewMatrix * vec4(pos, 1.0);
}
`

export const tentacleFragment = /* glsl */ `
uniform vec3 uColor;
uniform vec3 uTip;
uniform float uFade;
varying float vT;
varying vec2 vUv;

void main() {
  float edge = sin(vUv.x * 3.14159);
  vec3 col = mix(uColor, uTip, vT);
  col += edge * 0.12;
  float root = smoothstep(0.02, 0.16, vT);
  float alpha = (1.0 - vT * uFade) * (0.35 + edge * 0.5) * root;
  gl_FragColor = vec4(col, alpha);
}
`

export const glowVertex = /* glsl */ `
varying vec3 vNormal;
varying vec3 vView;

void main() {
  vNormal = normalize(normalMatrix * normal);
  vec4 mv = modelViewMatrix * vec4(position, 1.0);
  vView = normalize(-mv.xyz);
  gl_Position = projectionMatrix * mv;
}
`

export const glowFragment = /* glsl */ `
uniform vec3 uColor;
uniform float uPulse;
varying vec3 vNormal;
varying vec3 vView;

void main() {
  float core = pow(max(dot(vNormal, vView), 0.0), 1.6);
  float alpha = core * (0.5 + uPulse * 0.35);
  gl_FragColor = vec4(uColor, alpha);
}
`

export const faceVertex = /* glsl */ `
varying vec2 vUv;

void main() {
  vUv = uv;
  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
}
`

export const blushFragment = /* glsl */ `
uniform vec3 uColor;
varying vec2 vUv;

void main() {
  float d = distance(vUv, vec2(0.5));
  float alpha = smoothstep(0.5, 0.05, d) * 0.55;
  gl_FragColor = vec4(uColor, alpha);
}
`

export const waterVertex = /* glsl */ `
varying vec2 vUv;

void main() {
  vUv = uv;
  gl_Position = vec4(position.xy, 0.9999, 1.0);
}
`

export const waterFragment = /* glsl */ `
uniform float uTime;
uniform vec2 uResolution;
uniform vec3 uDeep;
uniform vec3 uShallow;
varying vec2 vUv;

float ray(vec2 uv, float x, float w, float slant) {
  float d = abs(uv.x - x + (1.0 - uv.y) * slant);
  return smoothstep(w, 0.0, d);
}

void main() {
  vec3 col = mix(uDeep, uShallow, pow(vUv.y, 1.7) * 0.85);
  float r = 0.0;
  r += ray(vUv, 0.08 + sin(uTime * 0.08 + 1.1) * 0.03, 0.05, -0.14) * 0.4;
  r += ray(vUv, 0.28 + sin(uTime * 0.11) * 0.04, 0.055, 0.2) * 0.5;
  r += ray(vUv, 0.5 + sin(uTime * 0.07 + 2.0) * 0.05, 0.035, -0.26) * 0.4;
  r += ray(vUv, 0.7 + sin(uTime * 0.09 + 4.0) * 0.05, 0.065, 0.18) * 0.42;
  r += ray(vUv, 0.9 + sin(uTime * 0.1 + 2.7) * 0.03, 0.05, -0.18) * 0.45;
  col += uShallow * r * pow(vUv.y, 1.8) * 0.7;
  float vig = smoothstep(1.45, 0.3, distance(vUv, vec2(0.5, 0.55)));
  col *= 0.65 + vig * 0.35;
  gl_FragColor = vec4(col, 1.0);
}
`

export const bubbleVertex = /* glsl */ `
uniform float uTime;
attribute float aSeed;
varying float vFade;

void main() {
  vec3 pos = position;
  float speed = 0.14 + aSeed * 0.22;
  float y = mod(pos.y + uTime * speed + aSeed * 7.0, 7.0) - 3.5;
  pos.y = y;
  pos.x += sin(uTime * (0.4 + aSeed) + aSeed * 20.0) * 0.14;
  vFade = smoothstep(3.5, 2.2, abs(y));
  vec4 mv = modelViewMatrix * vec4(pos, 1.0);
  gl_PointSize = (2.0 + aSeed * 5.0) * (3.2 / -mv.z);
  gl_Position = projectionMatrix * mv;
}
`

export const bubbleFragment = /* glsl */ `
uniform vec3 uColor;
varying float vFade;

void main() {
  vec2 c = gl_PointCoord - 0.5;
  float d = length(c);
  float ring = smoothstep(0.5, 0.32, d) - smoothstep(0.34, 0.12, d) * 0.6;
  gl_FragColor = vec4(uColor, ring * 0.5 * vFade);
}
`

export const crtVertex = /* glsl */ `
varying vec2 vUv;

void main() {
  vUv = uv;
  gl_Position = vec4(position.xy, 0.0, 1.0);
}
`

export const crtFragment = /* glsl */ `
uniform sampler2D uScene;
uniform float uTime;
uniform vec2 uResolution;
uniform float uStrength;
varying vec2 vUv;

float hash(vec2 p) {
  return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

vec2 barrel(vec2 uv, float k) {
  vec2 c = uv - 0.5;
  float r2 = dot(c, c);
  return 0.5 + c * (1.0 + k * r2);
}

void main() {
  vec2 uv = barrel(vUv, 0.06 * uStrength);
  vec2 border = smoothstep(0.0, 0.012, uv) * smoothstep(1.0, 0.988, uv);
  float frame = border.x * border.y;

  float shift = 0.0016 * uStrength * (0.6 + dot(uv - 0.5, uv - 0.5) * 3.0);
  float rr = texture2D(uScene, uv + vec2(shift, 0.0)).r;
  float gg = texture2D(uScene, uv).g;
  float bb = texture2D(uScene, uv - vec2(shift, 0.0)).b;
  vec3 col = vec3(rr, gg, bb);

  float line = sin(uv.y * uResolution.y * 1.35);
  col *= 1.0 - (0.5 - 0.5 * line) * 0.16 * uStrength;

  float px = mod(gl_FragCoord.x, 3.0);
  vec3 grille = vec3(
    px < 1.0 ? 1.05 : 0.97,
    px >= 1.0 && px < 2.0 ? 1.05 : 0.97,
    px >= 2.0 ? 1.05 : 0.97
  );
  col *= mix(vec3(1.0), grille, 0.5 * uStrength);

  float band = smoothstep(0.05, 0.0, abs(fract(uv.y * 0.5 - uTime * 0.04) - 0.5) - 0.42);
  col += band * 0.02 * uStrength;

  col += (hash(uv * uResolution.xy + uTime * 60.0) - 0.5) * 0.035 * uStrength;
  col *= 1.0 + sin(uTime * 9.0) * 0.006 * uStrength;

  float vig = smoothstep(0.95, 0.35, distance(uv, vec2(0.5)));
  col *= 0.72 + vig * 0.28;

  gl_FragColor = vec4(col * frame, 1.0);
}
`
