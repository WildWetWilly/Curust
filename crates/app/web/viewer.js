// Curust 3D view: build volume, models and layer preview, drawn with WebGL2.
"use strict";

const Mat4 = {
  ident() { return new Float32Array([1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,0,1]); },
  // Column-major multiply: a * b
  mul(a, b) {
    const r = new Float32Array(16);
    for (let c = 0; c < 4; c++) for (let rr = 0; rr < 4; rr++) {
      let s = 0;
      for (let k = 0; k < 4; k++) s += a[k * 4 + rr] * b[c * 4 + k];
      r[c * 4 + rr] = s;
    }
    return r;
  },
  persp(fovy, aspect, near, far) {
    const f = 1 / Math.tan(fovy / 2), nf = 1 / (near - far);
    return new Float32Array([f / aspect,0,0,0, 0,f,0,0, 0,0,(far + near) * nf,-1, 0,0,2 * far * near * nf,0]);
  },
  ortho(l, r, b, t, n, f) {
    return new Float32Array([2/(r-l),0,0,0, 0,2/(t-b),0,0, 0,0,-2/(f-n),0, -(r+l)/(r-l),-(t+b)/(t-b),-(f+n)/(f-n),1]);
  },
  lookAt(eye, target, up) {
    let zx = eye[0]-target[0], zy = eye[1]-target[1], zz = eye[2]-target[2];
    let l = Math.hypot(zx, zy, zz) || 1; zx/=l; zy/=l; zz/=l;
    let xx = up[1]*zz - up[2]*zy, xy = up[2]*zx - up[0]*zz, xz = up[0]*zy - up[1]*zx;
    l = Math.hypot(xx, xy, xz) || 1; xx/=l; xy/=l; xz/=l;
    const yx = zy*xz - zz*xy, yy = zz*xx - zx*xz, yz = zx*xy - zy*xx;
    return new Float32Array([xx,yx,zx,0, xy,yy,zy,0, xz,yz,zz,0,
      -(xx*eye[0]+xy*eye[1]+xz*eye[2]), -(yx*eye[0]+yy*eye[1]+yz*eye[2]), -(zx*eye[0]+zy*eye[1]+zz*eye[2]), 1]);
  },
  // Row-major 4x4 (as the server sends it) to column-major.
  fromRows(m) {
    const r = new Float32Array(16);
    for (let i = 0; i < 4; i++) for (let j = 0; j < 4; j++) r[j * 4 + i] = m[i * 4 + j];
    return r;
  },
  invert(m) {
    const inv = new Float32Array(16);
    inv[0] = m[5]*m[10]*m[15]-m[5]*m[11]*m[14]-m[9]*m[6]*m[15]+m[9]*m[7]*m[14]+m[13]*m[6]*m[11]-m[13]*m[7]*m[10];
    inv[4] = -m[4]*m[10]*m[15]+m[4]*m[11]*m[14]+m[8]*m[6]*m[15]-m[8]*m[7]*m[14]-m[12]*m[6]*m[11]+m[12]*m[7]*m[10];
    inv[8] = m[4]*m[9]*m[15]-m[4]*m[11]*m[13]-m[8]*m[5]*m[15]+m[8]*m[7]*m[13]+m[12]*m[5]*m[11]-m[12]*m[7]*m[9];
    inv[12] = -m[4]*m[9]*m[14]+m[4]*m[10]*m[13]+m[8]*m[5]*m[14]-m[8]*m[6]*m[13]-m[12]*m[5]*m[10]+m[12]*m[6]*m[9];
    inv[1] = -m[1]*m[10]*m[15]+m[1]*m[11]*m[14]+m[9]*m[2]*m[15]-m[9]*m[3]*m[14]-m[13]*m[2]*m[11]+m[13]*m[3]*m[10];
    inv[5] = m[0]*m[10]*m[15]-m[0]*m[11]*m[14]-m[8]*m[2]*m[15]+m[8]*m[3]*m[14]+m[12]*m[2]*m[11]-m[12]*m[3]*m[10];
    inv[9] = -m[0]*m[9]*m[15]+m[0]*m[11]*m[13]+m[8]*m[1]*m[15]-m[8]*m[3]*m[13]-m[12]*m[1]*m[11]+m[12]*m[3]*m[9];
    inv[13] = m[0]*m[9]*m[14]-m[0]*m[10]*m[13]-m[8]*m[1]*m[14]+m[8]*m[2]*m[13]+m[12]*m[1]*m[10]-m[12]*m[2]*m[9];
    inv[2] = m[1]*m[6]*m[15]-m[1]*m[7]*m[14]-m[5]*m[2]*m[15]+m[5]*m[3]*m[14]+m[13]*m[2]*m[7]-m[13]*m[3]*m[6];
    inv[6] = -m[0]*m[6]*m[15]+m[0]*m[7]*m[14]+m[4]*m[2]*m[15]-m[4]*m[3]*m[14]-m[12]*m[2]*m[7]+m[12]*m[3]*m[6];
    inv[10] = m[0]*m[5]*m[15]-m[0]*m[7]*m[13]-m[4]*m[1]*m[15]+m[4]*m[3]*m[13]+m[12]*m[1]*m[7]-m[12]*m[3]*m[5];
    inv[14] = -m[0]*m[5]*m[14]+m[0]*m[6]*m[13]+m[4]*m[1]*m[14]-m[4]*m[2]*m[13]-m[12]*m[1]*m[6]+m[12]*m[2]*m[5];
    inv[3] = -m[1]*m[6]*m[11]+m[1]*m[7]*m[10]+m[5]*m[2]*m[11]-m[5]*m[3]*m[10]-m[9]*m[2]*m[7]+m[9]*m[3]*m[6];
    inv[7] = m[0]*m[6]*m[11]-m[0]*m[7]*m[10]-m[4]*m[2]*m[11]+m[4]*m[3]*m[10]+m[8]*m[2]*m[7]-m[8]*m[3]*m[6];
    inv[11] = -m[0]*m[5]*m[11]+m[0]*m[7]*m[9]+m[4]*m[1]*m[11]-m[4]*m[3]*m[9]-m[8]*m[1]*m[7]+m[8]*m[3]*m[5];
    inv[15] = m[0]*m[5]*m[10]-m[0]*m[6]*m[9]-m[4]*m[1]*m[10]+m[4]*m[2]*m[9]+m[8]*m[1]*m[6]-m[8]*m[2]*m[5];
    let det = m[0]*inv[0]+m[1]*inv[4]+m[2]*inv[8]+m[3]*inv[12];
    if (det === 0) return Mat4.ident();
    det = 1 / det;
    for (let i = 0; i < 16; i++) inv[i] *= det;
    return inv;
  },
  apply(m, x, y, z) {
    const w = m[3]*x + m[7]*y + m[11]*z + m[15];
    return [(m[0]*x + m[4]*y + m[8]*z + m[12]) / w, (m[1]*x + m[5]*y + m[9]*z + m[13]) / w, (m[2]*x + m[6]*y + m[10]*z + m[14]) / w];
  },
};

// Cura's layer view colours (cura-light theme), indexed by PrintFeatureType.
const LINE_TYPE_COLORS = [
  [255, 255, 255], // NoneType
  [230, 0, 0],     // OuterWall
  [0, 230, 0],     // InnerWall
  [230, 230, 0],   // Skin
  [0, 230, 230],   // Support
  [0, 230, 230],   // SkirtBrim
  [230, 115, 0],   // Infill
  [0, 230, 230],   // SupportInfill
  [0, 0, 255],     // MoveUnretracted (combing)
  [128, 127, 255], // MoveRetracted
  [63, 127, 255],  // SupportInterface
  [0, 255, 255],   // PrimeTower
  [127, 255, 255], // MoveWhileRetracting
  [255, 127, 255], // MoveWhileUnretracting
  [200, 200, 200], // StationaryRetractUnretract
];
const LINE_TYPE_NAMES = ["Unknown", "Outer wall", "Inner wall", "Top / bottom", "Support", "Skirt / brim", "Infill", "Support infill",
  "Travel", "Retracted travel", "Support interface", "Prime tower", "Travel (retracting)", "Travel (priming)", "Retract"];
const isTravel = (t) => t === 8 || t === 9 || t === 12 || t === 13 || t === 0 || t === 14;

function hexToRgb(h) {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/i.exec(h || "");
  return m ? [parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16)] : [255, 201, 36];
}

const VS_MESH = `#version 300 es
in vec3 aPos; in vec3 aNormal;
uniform mat4 uModel, uView, uProj;
out vec3 vNormal; out vec3 vWorld;
void main() {
  vec4 w = uModel * vec4(aPos, 1.0);
  vWorld = w.xyz;
  vNormal = mat3(uModel) * aNormal;
  gl_Position = uProj * uView * w;
}`;
const FS_MESH = `#version 300 es
precision highp float;
in vec3 vNormal; in vec3 vWorld;
uniform vec3 uColor; uniform float uSelected; uniform float uOutside; uniform vec3 uEye; uniform float uOverhang;
out vec4 frag;
void main() {
  vec3 n = normalize(vNormal);
  if (!gl_FrontFacing) n = -n;
  vec3 l = normalize(vec3(0.35, 0.9, 0.55));
  vec3 v = normalize(uEye - vWorld);
  float diff = max(dot(n, l), 0.0);
  float spec = pow(max(dot(reflect(-l, n), v), 0.0), 24.0) * 0.18;
  vec3 base = uColor;
  if (uOverhang > 0.5 && n.y < -0.707 && vWorld.y > 0.25) base = mix(base, vec3(0.9, 0.12, 0.12), 0.75);
  vec3 c = base * (0.38 + 0.62 * diff) + vec3(spec);
  if (uSelected > 0.5) {
    // Selected: brighter, with a blue sheen towards the silhouette (the outline pass adds the edge).
    float rim = pow(1.0 - abs(dot(n, v)), 2.0);
    c = mix(c, vec3(1.0), 0.22);
    c = mix(c, vec3(0.20, 0.51, 1.0), 0.12 + 0.45 * rim);
  }
  if (uOutside > 0.5) {
    float s = step(0.5, fract((vWorld.x + vWorld.y + vWorld.z) * 0.25));
    c = mix(vec3(0.55), vec3(0.75), s) * (0.5 + 0.5 * diff);
  }
  if (vWorld.y < 0.0) c *= 0.55;
  frag = vec4(c, 1.0);
}`;
const VS_FLAT = `#version 300 es
in vec3 aPos; in vec4 aColor;
uniform mat4 uView, uProj;
out vec4 vColor;
void main() { vColor = aColor; gl_Position = uProj * uView * vec4(aPos, 1.0); }`;
const FS_FLAT = `#version 300 es
precision highp float;
in vec4 vColor; out vec4 frag;
void main() { frag = vColor; }`;
const VS_PICK = `#version 300 es
in vec3 aPos;
uniform mat4 uModel, uView, uProj;
void main() { gl_Position = uProj * uView * uModel * vec4(aPos, 1.0); }`;
const FS_PICK = `#version 300 es
precision highp float;
uniform vec3 uId; out vec4 frag;
void main() { frag = vec4(uId, 1.0); }`;
// Selection outline: a full-screen pass over the silhouette mask of the selected models.
const VS_QUAD = `#version 300 es
in vec2 aPos; out vec2 vUv;
void main() { vUv = aPos * 0.5 + 0.5; gl_Position = vec4(aPos, 0.0, 1.0); }`;
const FS_OUTLINE = `#version 300 es
precision highp float;
in vec2 vUv; uniform sampler2D uMask; uniform vec2 uTexel; uniform float uRadius; uniform vec3 uColor;
out vec4 frag;
void main() {
  float c = texture(uMask, vUv).r;
  float m = 0.0;
  for (int i = 0; i < 16; i++) {
    float a = float(i) * 0.3926991;
    for (int r = 1; r <= 3; r++) {
      vec2 o = vec2(cos(a), sin(a)) * uRadius * float(r) / 3.0;
      m = max(m, texture(uMask, vUv + o * uTexel).r);
    }
  }
  if (c > 0.5) { frag = vec4(uColor, 0.10); return; }
  if (m < 0.5) discard;
  frag = vec4(uColor, 1.0);
}`;
const VS_LAYER = `#version 300 es
in vec3 aPos; in vec3 aNormal; in vec4 aColor;
uniform mat4 uView, uProj;
out vec3 vNormal; out vec4 vColor;
void main() { vNormal = aNormal; vColor = aColor; gl_Position = uProj * uView * vec4(aPos, 1.0); }`;
const FS_LAYER = `#version 300 es
precision highp float;
in vec3 vNormal; in vec4 vColor;
uniform float uDim;
out vec4 frag;
void main() {
  vec3 n = normalize(vNormal);
  vec3 l = normalize(vec3(0.35, 0.9, 0.55));
  float d = 0.45 + 0.55 * max(abs(dot(n, l)), 0.0);
  frag = vec4(vColor.rgb * d * uDim, 1.0);
}`;

class Viewer {
  constructor(canvas) {
    this.canvas = canvas;
    const gl = canvas.getContext("webgl2", { antialias: true, preserveDrawingBuffer: true });
    if (!gl) throw new Error("WebGL2 is not available in this browser.");
    this.gl = gl;
    this.progMesh = this.program(VS_MESH, FS_MESH);
    this.progFlat = this.program(VS_FLAT, FS_FLAT);
    this.progPick = this.program(VS_PICK, FS_PICK);
    this.progLayer = this.program(VS_LAYER, FS_LAYER);
    this.progOutline = this.program(VS_QUAD, FS_OUTLINE);
    this.quadVao = gl.createVertexArray(); gl.bindVertexArray(this.quadVao);
    const qb = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, qb);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]), gl.STATIC_DRAW);
    const qa = this.progOutline.a("aPos"); gl.enableVertexAttribArray(qa); gl.vertexAttribPointer(qa, 2, gl.FLOAT, false, 0, 0);
    gl.bindVertexArray(null);
    this.meshes = new Map(); // id -> {vao, count, positions (Float32Array), version}
    this.nodes = [];
    this.selected = new Set();
    this.machine = null;
    this.yaw = -0.6; this.pitch = 0.55; this.dist = 420; this.target = [0, 40, 0];
    this.ortho = false;
    this.fov = 0.5;
    this.mode = "prepare";
    this.layers = null;
    this.layerMax = 0; this.layerMin = 0;
    this.colorScheme = "type";
    this.showTravel = false;
    this.overhang = true;
    this.extruderColors = [];
    this.dirty = true;
    this.dragOverride = null; // {id, world} while dragging
    this.resize();
    window.addEventListener("resize", () => { this.resize(); this.dirty = true; });
    const loop = () => { if (this.dirty) { this.dirty = false; this.draw(); } requestAnimationFrame(loop); };
    requestAnimationFrame(loop);
  }

  program(vs, fs) {
    const gl = this.gl;
    const mk = (t, src) => {
      const s = gl.createShader(t); gl.shaderSource(s, src); gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
      return s;
    };
    const p = gl.createProgram();
    gl.attachShader(p, mk(gl.VERTEX_SHADER, vs)); gl.attachShader(p, mk(gl.FRAGMENT_SHADER, fs));
    gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p));
    const u = {};
    const n = gl.getProgramParameter(p, gl.ACTIVE_UNIFORMS);
    for (let i = 0; i < n; i++) { const info = gl.getActiveUniform(p, i); u[info.name] = gl.getUniformLocation(p, info.name); }
    return { p, u, a: (name) => gl.getAttribLocation(p, name) };
  }

  resize() {
    const dpr = window.devicePixelRatio || 1;
    const r = this.canvas.getBoundingClientRect();
    this.canvas.width = Math.max(1, Math.round(r.width * dpr));
    this.canvas.height = Math.max(1, Math.round(r.height * dpr));
  }

  // ---- camera ----
  eye() {
    const cp = Math.cos(this.pitch), sp = Math.sin(this.pitch);
    return [this.target[0] + this.dist * cp * Math.sin(this.yaw), this.target[1] + this.dist * sp, this.target[2] + this.dist * cp * Math.cos(this.yaw)];
  }
  viewMatrix() { return Mat4.lookAt(this.eye(), this.target, [0, 1, 0]); }
  projMatrix() {
    const a = this.canvas.width / this.canvas.height;
    if (this.ortho) { const h = this.dist * 0.42; return Mat4.ortho(-h * a, h * a, -h, h, -5000, 5000); }
    return Mat4.persp(this.fov, a, 1, 20000);
  }
  setView(name) {
    const m = this.machine || { width: 200, depth: 200, height: 200 };
    // Frame the build plate (and a part of the volume) like Cura's default camera.
    const hView = Math.min(m.height, Math.max(m.width, m.depth) * 1.2);
    this.target = [0, hView * 0.32, 0];
    const r = 0.5 * Math.hypot(m.width, m.depth, hView);
    const aspect = Math.max(0.5, this.canvas.width / this.canvas.height);
    const fov = this.fov, hfov = 2 * Math.atan(Math.tan(fov / 2) * aspect);
    this.dist = (r / Math.sin(Math.min(fov, hfov) / 2)) * 0.82;
    if (name === "front") { this.yaw = 0; this.pitch = 0.08; }
    else if (name === "top") { this.yaw = 0; this.pitch = 1.5607; }
    else if (name === "left") { this.yaw = -Math.PI / 2; this.pitch = 0.08; }
    else if (name === "right") { this.yaw = Math.PI / 2; this.pitch = 0.08; }
    else { this.yaw = 0; this.pitch = 0.42; }
    this.dirty = true;
  }
  orbit(dx, dy) { this.yaw -= dx * 0.008; this.pitch = Math.max(-1.5, Math.min(1.56, this.pitch + dy * 0.008)); this.dirty = true; }
  pan(dx, dy) {
    const e = this.eye();
    const f = [this.target[0] - e[0], this.target[1] - e[1], this.target[2] - e[2]];
    const fl = Math.hypot(...f);
    const right = [-f[2], 0, f[0]]; const rl = Math.hypot(...right) || 1;
    const s = this.dist * 0.0016;
    this.target[0] -= (right[0] / rl) * dx * s;
    this.target[2] -= (right[2] / rl) * dx * s;
    this.target[1] += dy * s * Math.cos(this.pitch);
    void fl;
    this.dirty = true;
  }
  zoom(delta, mx, my) {
    const f = Math.exp(delta * 0.0012);
    // Zoom towards the mouse point on the plate.
    const hit = mx !== undefined ? this.rayPlane(mx, my, 0) : null;
    const nd = Math.max(20, Math.min(6000, this.dist * f));
    if (hit && f < 1) {
      const k = 1 - nd / this.dist;
      this.target[0] += (hit[0] - this.target[0]) * k * 0.6;
      this.target[2] += (hit[2] - this.target[2]) * k * 0.6;
    }
    this.dist = nd;
    this.dirty = true;
  }
  ray(mx, my) {
    const r = this.canvas.getBoundingClientRect();
    const x = ((mx - r.left) / r.width) * 2 - 1, y = 1 - ((my - r.top) / r.height) * 2;
    const inv = Mat4.invert(Mat4.mul(this.projMatrix(), this.viewMatrix()));
    const a = Mat4.apply(inv, x, y, -1), b = Mat4.apply(inv, x, y, 1);
    const d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]]; const l = Math.hypot(...d);
    return { o: a, d: [d[0] / l, d[1] / l, d[2] / l] };
  }
  rayPlane(mx, my, y0) {
    const { o, d } = this.ray(mx, my);
    if (Math.abs(d[1]) < 1e-9) return null;
    const t = (y0 - o[1]) / d[1];
    if (t < 0) return null;
    return [o[0] + d[0] * t, y0, o[2] + d[2] * t];
  }

  // ---- scene data ----
  setMachine(m) { this.machine = m; this.buildVolume(); this.dirty = true; }
  setNodes(nodes, extruderColors) { this.nodes = nodes; this.extruderColors = extruderColors; this.dirty = true; }
  setMesh(id, positions) {
    const gl = this.gl;
    const old = this.meshes.get(id);
    if (old) { gl.deleteVertexArray(old.vao); gl.deleteBuffer(old.vb); gl.deleteBuffer(old.nb); }
    const n = positions.length / 3;
    const normals = new Float32Array(positions.length);
    for (let i = 0; i < n; i += 3) {
      const ax = positions[i*3], ay = positions[i*3+1], az = positions[i*3+2];
      const ux = positions[i*3+3]-ax, uy = positions[i*3+4]-ay, uz = positions[i*3+5]-az;
      const vx = positions[i*3+6]-ax, vy = positions[i*3+7]-ay, vz = positions[i*3+8]-az;
      let nx = uy*vz-uz*vy, ny = uz*vx-ux*vz, nz = ux*vy-uy*vx; const l = Math.hypot(nx, ny, nz) || 1;
      nx/=l; ny/=l; nz/=l;
      for (let k = 0; k < 3; k++) { normals[(i+k)*3] = nx; normals[(i+k)*3+1] = ny; normals[(i+k)*3+2] = nz; }
    }
    const vao = gl.createVertexArray(); gl.bindVertexArray(vao);
    const vb = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, vb); gl.bufferData(gl.ARRAY_BUFFER, positions, gl.STATIC_DRAW);
    const ap = this.progMesh.a("aPos"); gl.enableVertexAttribArray(ap); gl.vertexAttribPointer(ap, 3, gl.FLOAT, false, 0, 0);
    const nb = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, nb); gl.bufferData(gl.ARRAY_BUFFER, normals, gl.STATIC_DRAW);
    const an = this.progMesh.a("aNormal"); gl.enableVertexAttribArray(an); gl.vertexAttribPointer(an, 3, gl.FLOAT, false, 0, 0);
    gl.bindVertexArray(null);
    this.meshes.set(id, { vao, vb, nb, count: n, positions });
    this.dirty = true;
  }
  dropMesh(id) {
    const m = this.meshes.get(id);
    if (!m) return;
    this.gl.deleteVertexArray(m.vao); this.gl.deleteBuffer(m.vb); this.gl.deleteBuffer(m.nb);
    this.meshes.delete(id);
  }

  flatBuffer(verts) {
    // verts: array of [x,y,z, r,g,b,a]
    const gl = this.gl;
    const data = new Float32Array(verts.length * 7);
    verts.forEach((v, i) => data.set(v, i * 7));
    const vao = gl.createVertexArray(); gl.bindVertexArray(vao);
    const b = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, b); gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
    const ap = this.progFlat.a("aPos"), ac = this.progFlat.a("aColor");
    gl.enableVertexAttribArray(ap); gl.vertexAttribPointer(ap, 3, gl.FLOAT, false, 28, 0);
    gl.enableVertexAttribArray(ac); gl.vertexAttribPointer(ac, 4, gl.FLOAT, false, 28, 12);
    gl.bindVertexArray(null);
    return { vao, b, count: verts.length };
  }

  buildVolume() {
    const m = this.machine;
    const gl = this.gl;
    for (const k of ["plate", "grid", "box", "areas"]) if (this[k]) { gl.deleteVertexArray(this[k].vao); gl.deleteBuffer(this[k].b); this[k] = null; }
    if (!m) return;
    const w = m.width, d = m.depth, h = m.height;
    const plate = [], grid = [], box = [], areas = [];
    const pc = [0.957, 0.957, 0.957, 1];
    const ell = m.shape === "elliptic";
    if (ell) {
      const n = 96;
      for (let i = 0; i < n; i++) {
        const a0 = (i / n) * Math.PI * 2, a1 = ((i + 1) / n) * Math.PI * 2;
        plate.push([0, -0.05, 0, ...pc], [Math.cos(a0) * w / 2, -0.05, Math.sin(a0) * d / 2, ...pc], [Math.cos(a1) * w / 2, -0.05, Math.sin(a1) * d / 2, ...pc]);
      }
    } else {
      const a = [-w/2, -0.05, -d/2], b = [w/2, -0.05, -d/2], c = [w/2, -0.05, d/2], e = [-w/2, -0.05, d/2];
      plate.push([...a, ...pc], [...b, ...pc], [...c, ...pc], [...a, ...pc], [...c, ...pc], [...e, ...pc]);
    }
    const inside = (x, z) => !ell || (x * x) / (w * w / 4) + (z * z) / (d * d / 4) <= 1.0001;
    for (let x = 0; x <= w / 2; x += 10) for (const s of x === 0 ? [1] : [1, -1]) {
      const xx = s * x; const major = Math.round(x) % 50 === 0;
      const col = major ? [0.71, 0.71, 0.71, 1] : [0.86, 0.86, 0.86, 1];
      let z0 = -d / 2, z1 = d / 2;
      if (ell) { const k = Math.sqrt(Math.max(0, 1 - (4 * xx * xx) / (w * w))); z0 = -k * d / 2; z1 = k * d / 2; }
      grid.push([xx, 0.01, z0, ...col], [xx, 0.01, z1, ...col]);
    }
    for (let z = 0; z <= d / 2; z += 10) for (const s of z === 0 ? [1] : [1, -1]) {
      const zz = s * z; const major = Math.round(z) % 50 === 0;
      const col = major ? [0.71, 0.71, 0.71, 1] : [0.86, 0.86, 0.86, 1];
      let x0 = -w / 2, x1 = w / 2;
      if (ell) { const k = Math.sqrt(Math.max(0, 1 - (4 * zz * zz) / (d * d))); x0 = -k * w / 2; x1 = k * w / 2; }
      grid.push([x0, 0.01, zz, ...col], [x1, 0.01, zz, ...col]);
    }
    void inside;
    const bc = [0.196, 0.51, 1.0, 0.85];
    if (ell) {
      const n = 96;
      for (const y of [0, h]) for (let i = 0; i < n; i++) {
        const a0 = (i / n) * Math.PI * 2, a1 = ((i + 1) / n) * Math.PI * 2;
        box.push([Math.cos(a0) * w / 2, y, Math.sin(a0) * d / 2, ...bc], [Math.cos(a1) * w / 2, y, Math.sin(a1) * d / 2, ...bc]);
      }
      for (const [x, z] of [[w / 2, 0], [-w / 2, 0], [0, d / 2], [0, -d / 2]]) box.push([x, 0, z, ...bc], [x, h, z, ...bc]);
    } else {
      const c = [[-w/2,-d/2],[w/2,-d/2],[w/2,d/2],[-w/2,d/2]];
      for (let i = 0; i < 4; i++) {
        const [x0, z0] = c[i], [x1, z1] = c[(i + 1) % 4];
        box.push([x0, 0, z0, ...bc], [x1, 0, z1, ...bc], [x0, h, z0, ...bc], [x1, h, z1, ...bc], [x0, 0, z0, ...bc], [x0, h, z0, ...bc]);
      }
    }
    // Axes at the origin corner (like Cura): X red, Y (depth) green, Z (up) blue.
    const ox = m.center_is_zero ? 0 : -w / 2, oz = m.center_is_zero ? 0 : d / 2;
    box.push([ox, 0.05, oz, 0.85, 0.12, 0.16, 1], [ox + 12, 0.05, oz, 0.85, 0.12, 0.16, 1]);
    box.push([ox, 0.05, oz, 0.14, 0.64, 0.29, 1], [ox, 0.05, oz - 12, 0.14, 0.64, 0.29, 1]);
    box.push([ox, 0.05, oz, 0.1, 0.43, 0.94, 1], [ox, 12, oz, 0.1, 0.43, 0.94, 1]);
    // Disallowed areas and the edge border.
    const ac = [0.0, 0.0, 0.0, 0.16];
    for (const a of m.disallowed_areas || []) {
      for (let i = 1; i + 1 < a.length; i++) areas.push([a[0][0], 0.02, a[0][1], ...ac], [a[i][0], 0.02, a[i][1], ...ac], [a[i + 1][0], 0.02, a[i + 1][1], ...ac]);
    }
    const bdr = m.border || 0;
    if (bdr > 0 && !ell) {
      const q = (x0, z0, x1, z1) => areas.push([x0, 0.02, z0, ...ac], [x1, 0.02, z0, ...ac], [x1, 0.02, z1, ...ac], [x0, 0.02, z0, ...ac], [x1, 0.02, z1, ...ac], [x0, 0.02, z1, ...ac]);
      q(-w/2, -d/2, w/2, -d/2 + bdr); q(-w/2, d/2 - bdr, w/2, d/2); q(-w/2, -d/2 + bdr, -w/2 + bdr, d/2 - bdr); q(w/2 - bdr, -d/2 + bdr, w/2, d/2 - bdr);
    }
    this.plate = this.flatBuffer(plate);
    this.grid = this.flatBuffer(grid);
    this.box = this.flatBuffer(box);
    this.areas = areas.length ? this.flatBuffer(areas) : null;
  }

  nodeMatrix(n) {
    if (this.dragOverride && this.dragOverride.id === n.id) return Mat4.fromRows(this.dragOverride.world);
    return Mat4.fromRows(n.world);
  }

  // ---- layer view ----
  setLayers(buf, machine, extruderColors) {
    const gl = this.gl;
    this.clearLayers();
    const dv = new DataView(buf);
    if (buf.byteLength < 8 || dv.getUint32(0, true) !== 0x594c5243) { return; }
    const nl = dv.getUint32(4, true);
    let p = 8;
    const ox = machine.center_is_zero ? 0 : -machine.width / 2, oz = machine.center_is_zero ? 0 : machine.depth / 2;
    const layers = [];
    let totalSegs = 0;
    const raw = [];
    let minFeed = Infinity, maxFeed = 0, minW = Infinity, maxW = 0;
    for (let li = 0; li < nl; li++) {
      const id = dv.getInt32(p, true), z = dv.getFloat32(p + 4, true), th = dv.getFloat32(p + 8, true), np = dv.getUint32(p + 12, true); p += 16;
      const paths = [];
      for (let k = 0; k < np; k++) {
        const ext = dv.getUint32(p, true), n = dv.getUint32(p + 4, true); p += 8;
        const pts = new Float32Array(buf, p, n * 3); p += n * 12;
        const types = new Uint8Array(buf, p, Math.max(0, n - 1)); p += Math.max(0, n - 1); while (p % 4) p++;
        const widths = new Float32Array(buf, p, Math.max(0, n - 1)); p += Math.max(0, n - 1) * 4;
        const feeds = new Float32Array(buf, p, Math.max(0, n - 1)); p += Math.max(0, n - 1) * 4;
        for (let s = 0; s < n - 1; s++) if (!isTravel(types[s])) {
          minFeed = Math.min(minFeed, feeds[s]); maxFeed = Math.max(maxFeed, feeds[s]);
          minW = Math.min(minW, widths[s]); maxW = Math.max(maxW, widths[s]);
        }
        paths.push({ ext, n, pts, types, widths, feeds });
        totalSegs += n - 1;
      }
      raw.push({ id, z, th, paths });
    }
    this.layerStats = { minFeed, maxFeed, minW, maxW };
    const full = totalSegs < 2500000;
    for (const L of raw) {
      // Build triangles: a top ribbon (and a vertical ribbon when affordable) per extrusion.
      let nExt = 0, nTrav = 0;
      for (const P of L.paths) for (let s = 0; s < P.n - 1; s++) { if (isTravel(P.types[s])) nTrav++; else nExt++; }
      const vPer = full ? 8 : 4, iPer = full ? 12 : 6;
      const pos = new Float32Array(nExt * vPer * 3), nor = new Int8Array(nExt * vPer * 4), col = new Uint8Array(nExt * vPer * 4);
      const idx = new Uint32Array(nExt * iPer);
      const tpos = new Float32Array(nTrav * 6), tcol = new Uint8Array(nTrav * 8);
      let v = 0, ii = 0, tv = 0;
      for (const P of L.paths) {
        for (let s = 0; s < P.n - 1; s++) {
          const t = P.types[s];
          const x0 = P.pts[s*3] + ox, z0 = -P.pts[s*3+1] + oz, y0 = P.pts[s*3+2];
          const x1 = P.pts[s*3+3] + ox, z1 = -P.pts[s*3+4] + oz, y1 = P.pts[s*3+5];
          let c = this.segmentColor(t, P.ext, P.feeds[s], P.widths[s], extruderColors);
          if (isTravel(t)) {
            tpos.set([x0, y0 + 0.02, z0, x1, y1 + 0.02, z1], tv * 6);
            tcol.set([...c, 255, ...c, 255], tv * 8); tv++;
            continue;
          }
          const dx = x1 - x0, dz = z1 - z0; const l = Math.hypot(dx, dz) || 1;
          const hw = Math.max(P.widths[s], 0.05) / 2;
          const px = -dz / l * hw, pz = dx / l * hw;
          const th = Math.max(L.th || 0.2, 0.05);
          const base = v;
          // top ribbon
          pos.set([x0 + px, y0, z0 + pz, x0 - px, y0, z0 - pz, x1 + px, y1, z1 + pz, x1 - px, y1, z1 - pz], v * 3);
          for (let k = 0; k < 4; k++) { nor.set([0, 127, 0, 0], (v + k) * 4); col.set([...c, 255], (v + k) * 4); }
          v += 4;
          idx.set([base, base + 1, base + 2, base + 1, base + 3, base + 2], ii); ii += 6;
          if (full) {
            const nx = Math.round(-dz / l * 127), nz = Math.round(dx / l * 127);
            pos.set([x0, y0, z0, x0, y0 - th * 0.85, z0, x1, y1, z1, x1, y1 - th * 0.85, z1], v * 3);
            const dc = c.map((x) => Math.round(x * 0.8));
            for (let k = 0; k < 4; k++) { nor.set([nx, 0, nz, 0], (v + k) * 4); col.set([...dc, 255], (v + k) * 4); }
            idx.set([v, v + 1, v + 2, v + 1, v + 3, v + 2], ii); ii += 6;
            v += 4;
          }
        }
      }
      const vao = gl.createVertexArray(); gl.bindVertexArray(vao);
      const bufs = [];
      const attr = (name, data, size, type, norm) => {
        const b = gl.createBuffer(); bufs.push(b); gl.bindBuffer(gl.ARRAY_BUFFER, b); gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
        const a = this.progLayer.a(name); if (a < 0) return; gl.enableVertexAttribArray(a); gl.vertexAttribPointer(a, size, type, norm, 0, 0);
      };
      attr("aPos", pos, 3, gl.FLOAT, false); attr("aNormal", nor, 4, gl.BYTE, true); attr("aColor", col, 4, gl.UNSIGNED_BYTE, true);
      const ib = gl.createBuffer(); bufs.push(ib); gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ib); gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, idx, gl.STATIC_DRAW);
      gl.bindVertexArray(null);
      let travel = null;
      if (tv) {
        const tvao = gl.createVertexArray(); gl.bindVertexArray(tvao);
        const b1 = gl.createBuffer(); bufs.push(b1); gl.bindBuffer(gl.ARRAY_BUFFER, b1); gl.bufferData(gl.ARRAY_BUFFER, tpos, gl.STATIC_DRAW);
        const ap = this.progFlat.a("aPos"); gl.enableVertexAttribArray(ap); gl.vertexAttribPointer(ap, 3, gl.FLOAT, false, 0, 0);
        const b2 = gl.createBuffer(); bufs.push(b2); gl.bindBuffer(gl.ARRAY_BUFFER, b2); gl.bufferData(gl.ARRAY_BUFFER, tcol, gl.STATIC_DRAW);
        const ac = this.progFlat.a("aColor"); gl.enableVertexAttribArray(ac); gl.vertexAttribPointer(ac, 4, gl.UNSIGNED_BYTE, true, 0, 0);
        gl.bindVertexArray(null);
        travel = { vao: tvao, count: tv * 2 };
      }
      layers.push({ id: L.id, z: L.z, vao, count: ii, bufs, travel, segs: nExt });
    }
    this.layers = layers;
    this.layerMin = 0;
    this.layerMax = layers.length - 1;
    this.dirty = true;
  }
  segmentColor(t, ext, feed, width, extruderColors) {
    if (this.colorScheme === "material") return extruderColors[ext] || [200, 200, 200];
    if (this.colorScheme === "speed" && !isTravel(t)) {
      const s = this.layerStats || { minFeed: 0, maxFeed: 1 };
      const k = s.maxFeed > s.minFeed ? (feed - s.minFeed) / (s.maxFeed - s.minFeed) : 0.5;
      return heat(k);
    }
    if (this.colorScheme === "width" && !isTravel(t)) {
      const s = this.layerStats || { minW: 0, maxW: 1 };
      const k = s.maxW > s.minW ? (width - s.minW) / (s.maxW - s.minW) : 0.5;
      return heat(k);
    }
    return LINE_TYPE_COLORS[t] || [255, 255, 255];
  }
  clearLayers() {
    const gl = this.gl;
    if (this.layers) for (const L of this.layers) { gl.deleteVertexArray(L.vao); if (L.travel) gl.deleteVertexArray(L.travel.vao); for (const b of L.bufs) gl.deleteBuffer(b); }
    this.layers = null;
    this.dirty = true;
  }

  // ---- drawing ----
  draw() {
    const gl = this.gl;
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.clearColor(0.925, 0.933, 0.945, 1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    gl.enable(gl.DEPTH_TEST);
    const view = this.viewMatrix(), proj = this.projMatrix(), eye = this.eye();
    // Build plate
    if (this.plate) {
      gl.useProgram(this.progFlat.p);
      gl.uniformMatrix4fv(this.progFlat.u.uView, false, view);
      gl.uniformMatrix4fv(this.progFlat.u.uProj, false, proj);
      gl.enable(gl.POLYGON_OFFSET_FILL); gl.polygonOffset(1, 1);
      gl.bindVertexArray(this.plate.vao); gl.drawArrays(gl.TRIANGLES, 0, this.plate.count);
      gl.disable(gl.POLYGON_OFFSET_FILL);
      gl.bindVertexArray(this.grid.vao); gl.drawArrays(gl.LINES, 0, this.grid.count);
      if (this.areas) {
        gl.enable(gl.BLEND); gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA); gl.depthMask(false);
        gl.bindVertexArray(this.areas.vao); gl.drawArrays(gl.TRIANGLES, 0, this.areas.count);
        gl.depthMask(true); gl.disable(gl.BLEND);
      }
    }
    if (this.mode === "preview" && this.layers) {
      gl.useProgram(this.progLayer.p);
      gl.uniformMatrix4fv(this.progLayer.u.uView, false, view);
      gl.uniformMatrix4fv(this.progLayer.u.uProj, false, proj);
      for (let i = this.layerMin; i <= this.layerMax && i < this.layers.length; i++) {
        const L = this.layers[i];
        gl.uniform1f(this.progLayer.u.uDim, i === this.layerMax ? 1.0 : 0.82);
        gl.bindVertexArray(L.vao); gl.drawElements(gl.TRIANGLES, L.count, gl.UNSIGNED_INT, 0);
      }
      if (this.showTravel) {
        gl.useProgram(this.progFlat.p);
        gl.uniformMatrix4fv(this.progFlat.u.uView, false, view);
        gl.uniformMatrix4fv(this.progFlat.u.uProj, false, proj);
        for (let i = this.layerMin; i <= this.layerMax && i < this.layers.length; i++) {
          const L = this.layers[i];
          if (L.travel) { gl.bindVertexArray(L.travel.vao); gl.drawArrays(gl.LINES, 0, L.travel.count); }
        }
      }
      // Ghost models
      gl.enable(gl.BLEND); gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA); gl.depthMask(false);
      gl.depthMask(true); gl.disable(gl.BLEND);
    } else {
      gl.useProgram(this.progMesh.p);
      gl.uniformMatrix4fv(this.progMesh.u.uView, false, view);
      gl.uniformMatrix4fv(this.progMesh.u.uProj, false, proj);
      gl.uniform3fv(this.progMesh.u.uEye, eye);
      for (const n of this.nodes) {
        const mesh = this.meshes.get(n.id);
        if (!mesh) continue;
        gl.uniformMatrix4fv(this.progMesh.u.uModel, false, this.nodeMatrix(n));
        let c = (this.extruderColors[n.extruder] || [255, 201, 36]).map((x) => x / 255);
        if (n.mesh_type !== "normal") c = [0.62, 0.66, 0.72];
        gl.uniform3fv(this.progMesh.u.uColor, c);
        gl.uniform1f(this.progMesh.u.uSelected, this.selected.has(n.id) ? 1 : 0);
        gl.uniform1f(this.progMesh.u.uOutside, n.outside ? 1 : 0);
        gl.uniform1f(this.progMesh.u.uOverhang, this.overhang && n.mesh_type === "normal" ? 1 : 0);
        gl.bindVertexArray(mesh.vao); gl.drawArrays(gl.TRIANGLES, 0, mesh.count);
      }
      if (this.nodes.some((n) => this.selected.has(n.id) && this.meshes.has(n.id))) this.drawOutline(view, proj);
    }
    // Volume outline last (on top of the plate).
    if (this.box) {
      gl.useProgram(this.progFlat.p);
      gl.uniformMatrix4fv(this.progFlat.u.uView, false, view);
      gl.uniformMatrix4fv(this.progFlat.u.uProj, false, proj);
      gl.bindVertexArray(this.box.vao); gl.drawArrays(gl.LINES, 0, this.box.count);
    }
    gl.bindVertexArray(null);
  }

  // Blue outline around the selected models (like Cura's selection).
  drawOutline(view, proj) {
    const gl = this.gl;
    const w = this.canvas.width, h = this.canvas.height;
    if (!this.maskFb || this.maskW !== w || this.maskH !== h) {
      if (this.maskFb) { gl.deleteFramebuffer(this.maskFb); gl.deleteTexture(this.maskTex); }
      this.maskTex = gl.createTexture();
      gl.bindTexture(gl.TEXTURE_2D, this.maskTex);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      this.maskFb = gl.createFramebuffer();
      gl.bindFramebuffer(gl.FRAMEBUFFER, this.maskFb);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, this.maskTex, 0);
      this.maskW = w; this.maskH = h;
    }
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.maskFb);
    gl.viewport(0, 0, w, h);
    gl.clearColor(0, 0, 0, 0); gl.clear(gl.COLOR_BUFFER_BIT);
    gl.disable(gl.DEPTH_TEST);
    gl.useProgram(this.progPick.p);
    gl.uniformMatrix4fv(this.progPick.u.uView, false, view);
    gl.uniformMatrix4fv(this.progPick.u.uProj, false, proj);
    gl.uniform3f(this.progPick.u.uId, 1, 1, 1);
    for (const n of this.nodes) {
      const mesh = this.meshes.get(n.id);
      if (!mesh || !this.selected.has(n.id)) continue;
      gl.uniformMatrix4fv(this.progPick.u.uModel, false, this.nodeMatrix(n));
      gl.bindVertexArray(mesh.vao); gl.drawArrays(gl.TRIANGLES, 0, mesh.count);
    }
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.viewport(0, 0, w, h);
    gl.enable(gl.BLEND); gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.useProgram(this.progOutline.p);
    gl.activeTexture(gl.TEXTURE0); gl.bindTexture(gl.TEXTURE_2D, this.maskTex);
    gl.uniform1i(this.progOutline.u.uMask, 0);
    gl.uniform2f(this.progOutline.u.uTexel, 1 / w, 1 / h);
    gl.uniform1f(this.progOutline.u.uRadius, 2.5 * (window.devicePixelRatio || 1));
    gl.uniform3f(this.progOutline.u.uColor, 0.196, 0.51, 1.0);
    gl.bindVertexArray(this.quadVao); gl.drawArrays(gl.TRIANGLES, 0, 6);
    gl.disable(gl.BLEND);
    gl.enable(gl.DEPTH_TEST);
  }

  // GPU picking: returns the node id under the mouse, or null.
  pick(mx, my) {
    const gl = this.gl;
    const w = this.canvas.width, h = this.canvas.height;
    if (!this.pickFb || this.pickW !== w || this.pickH !== h) {
      if (this.pickFb) { gl.deleteFramebuffer(this.pickFb); gl.deleteRenderbuffer(this.pickRb); gl.deleteRenderbuffer(this.pickDb); }
      this.pickFb = gl.createFramebuffer(); this.pickRb = gl.createRenderbuffer(); this.pickDb = gl.createRenderbuffer();
      gl.bindRenderbuffer(gl.RENDERBUFFER, this.pickRb); gl.renderbufferStorage(gl.RENDERBUFFER, gl.RGBA8, w, h);
      gl.bindRenderbuffer(gl.RENDERBUFFER, this.pickDb); gl.renderbufferStorage(gl.RENDERBUFFER, gl.DEPTH_COMPONENT24, w, h);
      gl.bindFramebuffer(gl.FRAMEBUFFER, this.pickFb);
      gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.RENDERBUFFER, this.pickRb);
      gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.DEPTH_ATTACHMENT, gl.RENDERBUFFER, this.pickDb);
      this.pickW = w; this.pickH = h;
    }
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.pickFb);
    gl.viewport(0, 0, w, h);
    gl.clearColor(0, 0, 0, 0); gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    gl.enable(gl.DEPTH_TEST);
    gl.useProgram(this.progPick.p);
    gl.uniformMatrix4fv(this.progPick.u.uView, false, this.viewMatrix());
    gl.uniformMatrix4fv(this.progPick.u.uProj, false, this.projMatrix());
    this.nodes.forEach((n, i) => {
      const mesh = this.meshes.get(n.id); if (!mesh) return;
      const k = i + 1;
      gl.uniform3f(this.progPick.u.uId, (k & 255) / 255, ((k >> 8) & 255) / 255, ((k >> 16) & 255) / 255);
      gl.uniformMatrix4fv(this.progPick.u.uModel, false, this.nodeMatrix(n));
      gl.bindVertexArray(mesh.vao); gl.drawArrays(gl.TRIANGLES, 0, mesh.count);
    });
    const r = this.canvas.getBoundingClientRect();
    const px = Math.round((mx - r.left) * (w / r.width)), py = Math.round(h - (my - r.top) * (h / r.height));
    const out = new Uint8Array(4);
    gl.readPixels(px, py, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, out);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.bindVertexArray(null);
    this.dirty = true;
    const k = out[0] + (out[1] << 8) + (out[2] << 16);
    return k > 0 && this.nodes[k - 1] ? this.nodes[k - 1].id : null;
  }

  // CPU ray cast against one node; returns the world position of the nearest hit.
  pickPoint(id, mx, my) {
    const n = this.nodes.find((x) => x.id === id); const mesh = this.meshes.get(id);
    if (!n || !mesh) return null;
    const M = this.nodeMatrix(n);
    const { o, d } = this.ray(mx, my);
    const P = mesh.positions;
    let best = Infinity;
    const tr = (i) => Mat4.apply(M, P[i*3], P[i*3+1], P[i*3+2]);
    for (let i = 0; i < P.length / 3; i += 3) {
      const a = tr(i), b = tr(i + 1), c = tr(i + 2);
      const e1 = [b[0]-a[0], b[1]-a[1], b[2]-a[2]], e2 = [c[0]-a[0], c[1]-a[1], c[2]-a[2]];
      const pv = [d[1]*e2[2]-d[2]*e2[1], d[2]*e2[0]-d[0]*e2[2], d[0]*e2[1]-d[1]*e2[0]];
      const det = e1[0]*pv[0]+e1[1]*pv[1]+e1[2]*pv[2];
      if (Math.abs(det) < 1e-12) continue;
      const inv = 1 / det;
      const tv = [o[0]-a[0], o[1]-a[1], o[2]-a[2]];
      const u = (tv[0]*pv[0]+tv[1]*pv[1]+tv[2]*pv[2]) * inv; if (u < 0 || u > 1) continue;
      const q = [tv[1]*e1[2]-tv[2]*e1[1], tv[2]*e1[0]-tv[0]*e1[2], tv[0]*e1[1]-tv[1]*e1[0]];
      const v = (d[0]*q[0]+d[1]*q[1]+d[2]*q[2]) * inv; if (v < 0 || u + v > 1) continue;
      const t = (e2[0]*q[0]+e2[1]*q[1]+e2[2]*q[2]) * inv;
      if (t > 0 && t < best) best = t;
    }
    return best < Infinity ? [o[0] + d[0] * best, o[1] + d[1] * best, o[2] + d[2] * best] : null;
  }

  // CPU ray cast against one node (for "lay flat on face"); returns the world normal.
  pickFace(id, mx, my) {
    const n = this.nodes.find((x) => x.id === id); const mesh = this.meshes.get(id);
    if (!n || !mesh) return null;
    const M = this.nodeMatrix(n);
    const { o, d } = this.ray(mx, my);
    const P = mesh.positions;
    let best = Infinity, bestN = null;
    const tr = (i) => Mat4.apply(M, P[i*3], P[i*3+1], P[i*3+2]);
    for (let i = 0; i < P.length / 3; i += 3) {
      const a = tr(i), b = tr(i + 1), c = tr(i + 2);
      const e1 = [b[0]-a[0], b[1]-a[1], b[2]-a[2]], e2 = [c[0]-a[0], c[1]-a[1], c[2]-a[2]];
      const pv = [d[1]*e2[2]-d[2]*e2[1], d[2]*e2[0]-d[0]*e2[2], d[0]*e2[1]-d[1]*e2[0]];
      const det = e1[0]*pv[0]+e1[1]*pv[1]+e1[2]*pv[2];
      if (Math.abs(det) < 1e-12) continue;
      const inv = 1 / det;
      const tv = [o[0]-a[0], o[1]-a[1], o[2]-a[2]];
      const u = (tv[0]*pv[0]+tv[1]*pv[1]+tv[2]*pv[2]) * inv; if (u < 0 || u > 1) continue;
      const q = [tv[1]*e1[2]-tv[2]*e1[1], tv[2]*e1[0]-tv[0]*e1[2], tv[0]*e1[1]-tv[1]*e1[0]];
      const v = (d[0]*q[0]+d[1]*q[1]+d[2]*q[2]) * inv; if (v < 0 || u + v > 1) continue;
      const t = (e2[0]*q[0]+e2[1]*q[1]+e2[2]*q[2]) * inv;
      if (t > 0 && t < best) {
        best = t;
        let nn = [e1[1]*e2[2]-e1[2]*e2[1], e1[2]*e2[0]-e1[0]*e2[2], e1[0]*e2[1]-e1[1]*e2[0]];
        const l = Math.hypot(...nn) || 1; nn = nn.map((x) => x / l);
        // Mirrored models flip the winding; make the normal face the viewer.
        if (nn[0]*d[0]+nn[1]*d[1]+nn[2]*d[2] > 0) nn = nn.map((x) => -x);
        bestN = nn;
      }
    }
    return bestN;
  }
}

function heat(k) {
  k = Math.max(0, Math.min(1, k));
  // blue -> cyan -> green -> yellow -> red
  const stops = [[30, 60, 230], [0, 200, 220], [40, 200, 60], [240, 220, 0], [230, 40, 20]];
  const f = k * (stops.length - 1); const i = Math.min(stops.length - 2, Math.floor(f)); const t = f - i;
  return stops[i].map((a, j) => Math.round(a + (stops[i + 1][j] - a) * t));
}

window.Viewer = Viewer;
window.Mat4 = Mat4;
window.LINE_TYPE_COLORS = LINE_TYPE_COLORS;
window.LINE_TYPE_NAMES = LINE_TYPE_NAMES;
window.hexToRgb = hexToRgb;
window.heat = heat;
