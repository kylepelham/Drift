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
`;

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
`;

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
`;

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
`;

export const glowVertex = /* glsl */ `
varying vec3 vNormal;
varying vec3 vView;

void main() {
  vNormal = normalize(normalMatrix * normal);
  vec4 mv = modelViewMatrix * vec4(position, 1.0);
  vView = normalize(-mv.xyz);
  gl_Position = projectionMatrix * mv;
}
`;

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
`;

export const faceVertex = /* glsl */ `
varying vec2 vUv;

void main() {
  vUv = uv;
  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
}
`;

export const blushFragment = /* glsl */ `
uniform vec3 uColor;
varying vec2 vUv;

void main() {
  float d = distance(vUv, vec2(0.5));
  float alpha = smoothstep(0.5, 0.05, d) * 0.55;
  gl_FragColor = vec4(uColor, alpha);
}
`;
