// Pulse: the reviewers' view of a user's heartbeats.
//
// Reads /api/review/users and /api/review/timeline (or, for the hosted
// example, window.PULSE_DEMO from demo-data.js) and draws:
// - the EKG strip: per device, keys per second up and mouse per second
//   down, credit behind, prompts as spikes, response ends as ticks, late
//   heartbeats hatched, flags as brackets on top
// - the rhythm fingerprint: a Poincaré plot of each active second against
//   the next
// - the reading clock: response length against the time to the next prompt
// - the flags list
// Data goes into the page with textContent and canvas only.
"use strict";

const DEMO = window.PULSE_DEMO || null;
const $ = (id) => document.getElementById(id);
const MINUTE = 60000;
const KEY_MAX = 20; // keys per second at the top of a lane (sqrt scale)
const MOUSE_MAX = 60;
const SEVERITY = { high: 0, medium: 1, low: 2 };

const state = {
  userId: null,
  from: 0,
  to: 0,
  data: null,
  model: null,
  view: null, // { from, to } in ms
  channel: "keys",
  selected: null, // flag index
  hover: null,
  brush: null,
};

// ---------------------------------------------------------------------------
// Helpers

function el(tag, cls, text) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

function duration(ms) {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

function number(n) {
  return Math.round(n).toLocaleString("en-US");
}

function clock(ms, seconds) {
  const d = new Date(ms);
  const hm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  return seconds ? `${hm}:${String(d.getSeconds()).padStart(2, "0")}` : hm;
}

function day(ms) {
  return new Date(ms).toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short" });
}

function span(from, to) {
  const sameDay = new Date(from).toDateString() === new Date(to - 1).toDateString();
  const fine = to - from < 3 * MINUTE;
  return sameDay
    ? `${day(from)}, ${clock(from, fine)} to ${clock(to, fine)}`
    : `${day(from)} ${clock(from)} to ${day(to)} ${clock(to)}`;
}

function css(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function alpha(color, a) {
  // Accepts #rrggbb or rgba(); returns the colour at opacity `a`.
  if (color.startsWith("#") && color.length === 7) {
    const n = parseInt(color.slice(1), 16);
    return `rgba(${n >> 16}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
  }
  const m = color.match(/rgba?\(([^)]+)\)/);
  if (m) {
    const [r, g, b] = m[1].split(",").map((x) => parseFloat(x));
    return `rgba(${r}, ${g}, ${b}, ${a})`;
  }
  return color;
}

function colors() {
  return {
    text: css("--text"),
    pink: css("--pink"),
    muted: css("--muted"),
    bubble: css("--bubble"),
    mouse: css("--mouse"),
    alert: css("--alert"),
    amber: css("--amber"),
    bg: css("--bg"),
  };
}

/** Size a canvas to its CSS width (and `height` CSS px), for the screen's
 * pixel ratio. Returns the 2D context scaled to CSS px. */
function fit(canvas, height) {
  const ratio = window.devicePixelRatio || 1;
  const width = canvas.clientWidth || canvas.parentElement.clientWidth;
  if (height === undefined) height = canvas.clientHeight || width;
  canvas.style.height = `${height}px`;
  canvas.width = Math.round(width * ratio);
  canvas.height = Math.round(height * ratio);
  const ctx = canvas.getContext("2d");
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  ctx.clearRect(0, 0, width, height);
  return { ctx, width, height };
}

// ---------------------------------------------------------------------------
// Data: the API, or the demo's data

async function getJson(url) {
  const response = await fetch(url, { credentials: "same-origin" });
  if (response.status === 401) throw new Error("logged out");
  if (response.status === 403) throw new Error("forbidden");
  if (!response.ok) throw new Error(`the server answered ${response.status}`);
  return response.json();
}

function demoNow() {
  return Math.max(...Object.values(DEMO.timelines).map((t) => t.to));
}

async function searchUsers(q) {
  if (DEMO) {
    const needle = q.trim().toLowerCase();
    return DEMO.users.filter(
      (u) =>
        !needle ||
        (u.name || "").toLowerCase().includes(needle) ||
        (u.slack_id || "").toLowerCase().includes(needle) ||
        String(u.id) === needle,
    );
  }
  return getJson(`/api/review/users?q=${encodeURIComponent(q)}`);
}

async function loadTimeline(user, from, to) {
  if (!DEMO) return getJson(`/api/review/timeline?user=${user}&from=${from}&to=${to}`);
  const full = DEMO.timelines[String(user)];
  if (!full) throw new Error("no such user in the example");
  const heartbeats = full.heartbeats.filter((h) => h.minute_ms >= from && h.minute_ms < to);
  const flags = full.flags.filter((f) => f.to_ms > from && f.from_ms < to);
  return { ...full, from, to, heartbeats, flags, summary: summarize(heartbeats, flags) };
}

/** The API's summary, recomputed (the demo filters its data by range). */
function summarize(heartbeats, flags) {
  const sum = (f) => heartbeats.reduce((n, h) => n + f(h), 0);
  const total = (a) => (a ? a.reduce((n, x) => n + x, 0) : 0);
  // Per-second counts exist only in v9 and v10 heartbeats; else the slots'.
  const slotTotal = (h, f) => (h.slots || []).reduce((n, s) => n + s[f], 0);
  const severities = { high: 0, medium: 0, low: 0 };
  for (const f of flags) severities[f.severity] = (severities[f.severity] || 0) + 1;
  return {
    heartbeats: heartbeats.length,
    credited_ms: sum((h) => h.credit_ms || 0),
    agent_ms: sum((h) => h.ai_ms || 0),
    keys: sum((h) => (h.keys ? total(h.keys) : slotTotal(h, "keys"))),
    mouse: sum((h) => (h.mouse ? total(h.mouse) : slotTotal(h, "mouse"))),
    injected: sum((h) => (h.injected ? h.injected.keys + h.injected.mouse : 0)),
    prompts: sum((h) => h.prompts.submitted),
    scripted_prompts: sum((h) => h.prompts.scripted || 0),
    offline: sum((h) => (h.offline ? 1 : 0)),
    stale: sum((h) => (h.stale ? 1 : 0)),
    flags: severities,
  };
}

// ---------------------------------------------------------------------------
// The model: per device, per-second arrays over the requested range

function buildModel(data) {
  const from = Math.floor(data.from / 1000) * 1000;
  const to = Math.max(from + 1000, Math.ceil(data.to / 1000) * 1000);
  const seconds = Math.round((to - from) / 1000);
  const lanes = new Map();
  const lane = (id, os) => {
    if (!lanes.has(id)) {
      lanes.set(id, {
        id,
        os: os || "",
        keys: new Uint16Array(seconds),
        mouse: new Uint16Array(seconds),
        has: new Uint8Array(seconds), // 1 measured, 2 spread from a slot
        minutes: new Map(),
        prompts: [],
        responses: [],
      });
    }
    return lanes.get(id);
  };
  for (const d of data.devices || []) lane(d.id, d.os);

  let first = Infinity;
  let last = -Infinity;
  for (const h of data.heartbeats) {
    const l = lane(h.device, h.os);
    if (!l.os && h.os) l.os = h.os;
    first = Math.min(first, h.minute_ms);
    last = Math.max(last, h.minute_ms + MINUTE);

    let m = l.minutes.get(h.minute_ms);
    if (!m) {
      m = { minute: h.minute_ms, hbs: [], credit: 0, multiplier: 0, running: false, offline: false, stale: false, scripted: 0 };
      l.minutes.set(h.minute_ms, m);
    }
    m.hbs.push(h);
    m.credit += h.credit_ms || 0;
    m.multiplier = Math.max(m.multiplier, h.multiplier || 0);
    m.running = m.running || h.agent_running;
    m.offline = m.offline || h.offline;
    m.stale = m.stale || h.stale;
    m.scripted += h.prompts.scripted || 0;

    // Input is the whole device's, repeated in every session's heartbeat.
    const base = Math.round((h.minute_ms - from) / 1000);
    for (let i = 0; i < 60; i++) {
      const s = base + i;
      if (s < 0 || s >= seconds) continue;
      let k;
      let mo;
      let kind = 1;
      if (h.keys && h.mouse) {
        k = h.keys[i] || 0;
        mo = h.mouse[i] || 0;
      } else {
        const slot = h.slots && h.slots[Math.floor(i / 10)];
        if (!slot) continue;
        k = Math.round(slot.keys / 10);
        mo = Math.round(slot.mouse / 10);
        kind = 2;
      }
      l.keys[s] = Math.max(l.keys[s], k);
      l.mouse[s] = Math.max(l.mouse[s], mo);
      if (!l.has[s] || kind === 1) l.has[s] = kind;
    }
    for (const at of h.prompts.at || []) {
      l.prompts.push({ t: h.minute_ms + at * 1000 + 500, hb: h, scripted: (h.prompts.scripted || 0) > 0 });
    }
    for (const r of (h.responses && h.responses.at) || []) {
      l.responses.push({ t: h.minute_ms + r.second * 1000 + 500, chars: r.chars, hb: h });
    }
  }
  for (const l of lanes.values()) {
    l.prompts.sort((a, b) => a.t - b.t);
    l.responses.sort((a, b) => a.t - b.t);
    l.minuteList = [...l.minutes.values()].sort((a, b) => a.minute - b.minute);
  }
  const extent =
    first < Infinity
      ? padded(first, last, 0.015)
      : { from: data.from, to: data.to };
  return { from, to, seconds, lanes: [...lanes.values()], extent };
}

function padded(from, to, share) {
  const pad = Math.max(30000, (to - from) * share);
  return { from: from - pad, to: to + pad };
}

// ---------------------------------------------------------------------------
// The EKG strip

const LANE = 132; // CSS px per device
const AXIS = 24;
const FLAG_ROW = 11;
const FLAG_ROWS_MAX = 5;

function flagRows(flags, view, width) {
  // Greedy interval packing so brackets don't overlap.
  const ends = [];
  const rows = flags.map((f) => {
    const x1 = xOf(f.from_ms, view, width);
    let row = ends.findIndex((end) => end < x1 - 4);
    if (row === -1) {
      row = Math.min(ends.length, FLAG_ROWS_MAX - 1);
      if (row === ends.length) ends.push(-Infinity);
    }
    ends[row] = Math.max(ends[row], xOf(f.to_ms, view, width));
    return row;
  });
  return { rows, count: Math.max(1, ends.length) };
}

function xOf(t, view, width) {
  return ((t - view.from) / (view.to - view.from)) * width;
}

function layout(width) {
  const flags = state.data ? state.data.flags : [];
  const packed = flagRows(flags, state.view, width);
  const top = 8 + packed.count * FLAG_ROW + 6;
  const lanes = state.model.lanes.length || 1;
  return { top, packed, height: top + lanes * LANE + AXIS };
}

function drawStrip() {
  const canvas = $("strip");
  if (!state.model) return;
  const width = canvas.parentElement.clientWidth;
  const lay = layout(width);
  const { ctx } = fit(canvas, lay.height);
  const c = colors();
  const view = state.view;
  const x = (t) => xOf(t, view, width);
  const model = state.model;

  // Selected flag: shade its time behind everything.
  const selected = state.selected !== null ? state.data.flags[state.selected] : null;
  if (selected) {
    ctx.fillStyle = alpha(c.alert, 0.09);
    const x1 = Math.max(0, x(selected.from_ms));
    const x2 = Math.min(width, x(selected.to_ms));
    ctx.fillRect(x1, lay.top - 4, Math.max(2, x2 - x1), model.lanes.length * LANE + 4);
  }

  const hatch = hatchPattern(ctx, c.muted);
  model.lanes.forEach((lane, i) => {
    const top = lay.top + i * LANE;
    drawLane(ctx, lane, top, width, x, c, hatch);
  });

  drawFlags(ctx, lay, width, x, c);
  drawAxis(ctx, lay.height - AXIS, width, c);

  if (state.brush) {
    const [a, b] = [state.brush.x0, state.brush.x1].sort((p, q) => p - q);
    ctx.fillStyle = alpha(c.pink, 0.16);
    ctx.fillRect(a, lay.top - 4, b - a, model.lanes.length * LANE + 4);
    ctx.strokeStyle = c.pink;
    ctx.lineWidth = 1;
    ctx.strokeRect(a + 0.5, lay.top - 3.5, b - a - 1, model.lanes.length * LANE + 3);
  }
  if (state.hover && !state.brush) {
    ctx.strokeStyle = alpha(c.text, 0.35);
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(Math.round(state.hover.x) + 0.5, lay.top - 4);
    ctx.lineTo(Math.round(state.hover.x) + 0.5, lay.top + model.lanes.length * LANE);
    ctx.stroke();
  }
  drawOverview();
}

function hatchPattern(ctx, color) {
  const tile = document.createElement("canvas");
  tile.width = 8;
  tile.height = 8;
  const t = tile.getContext("2d");
  t.strokeStyle = alpha(color, 0.5);
  t.lineWidth = 1.2;
  t.beginPath();
  t.moveTo(0, 8);
  t.lineTo(8, 0);
  t.stroke();
  return ctx.createPattern(tile, "repeat");
}

function drawLane(ctx, lane, top, width, x, c, hatch) {
  const view = state.view;
  const keysH = 66;
  const mouseH = 40;
  const base = top + 18 + keysH; // keys above, mouse below
  const bottom = top + LANE - 6;

  // Lane label.
  ctx.fillStyle = c.muted;
  ctx.font = "600 12px Fredoka, system-ui, sans-serif";
  ctx.textBaseline = "top";
  const name = `${osName(lane.os)} ${lane.id}`;
  ctx.fillText(name, 4, top + 2);

  // Credit and running, per minute.
  for (const m of lane.minuteList) {
    if (m.minute + MINUTE < view.from || m.minute > view.to) continue;
    const x1 = x(m.minute);
    const w = Math.max(1, x(m.minute + MINUTE) - x1);
    if (m.credit > 0) {
      ctx.fillStyle = alpha(c.pink, 0.05 + 0.2 * Math.min(1, m.credit / MINUTE));
      ctx.fillRect(x1, top + 16, w, base - top - 16);
    }
    if (m.running) {
      ctx.fillStyle = alpha(c.text, 0.22);
      ctx.fillRect(x1, base - 1, w, 3);
    }
    if (m.stale || m.offline) {
      ctx.fillStyle = hatch;
      ctx.globalAlpha = m.stale ? 1 : 0.55;
      ctx.fillRect(x1, top + 16, w, bottom - top - 16);
      ctx.globalAlpha = 1;
    }
  }

  // Baseline.
  ctx.fillStyle = alpha(c.text, 0.12);
  ctx.fillRect(0, base, width, 1);

  trace(ctx, lane, lane.keys, base, -keysH, KEY_MAX, c.pink, width);
  trace(ctx, lane, lane.mouse, base + 2, mouseH, MOUSE_MAX, c.mouse, width);

  // Prompts: R-peaks from the baseline to the top of the lane. When they
  // crowd (a prompt every few pixels), they thin out so the traces stay
  // readable.
  const shown = lane.prompts.filter((p) => p.t >= view.from && p.t <= view.to);
  const dense = shown.length > width / 8;
  for (const p of shown) {
    const px = Math.round(x(p.t)) + 0.5;
    const color = p.scripted ? c.alert : c.text;
    ctx.strokeStyle = dense ? alpha(color, 0.35) : color;
    ctx.lineWidth = dense ? 1 : 2;
    ctx.beginPath();
    ctx.moveTo(px, base);
    ctx.lineTo(px, top + 20);
    ctx.stroke();
    if (dense) continue;
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(px, top + 20, 3, 0, Math.PI * 2);
    ctx.fill();
  }
  // Response ends: small triangles on the baseline, bigger for longer.
  ctx.fillStyle = c.mouse;
  for (const r of lane.responses) {
    if (r.t < view.from || r.t > view.to) continue;
    const px = x(r.t);
    const s = 3 + Math.min(4, Math.log10(Math.max(10, r.chars)) - 1);
    ctx.beginPath();
    ctx.moveTo(px, base + 2);
    ctx.lineTo(px - s, base + 2 + s * 1.6);
    ctx.lineTo(px + s, base + 2 + s * 1.6);
    ctx.closePath();
    ctx.fill();
  }
}

/** Draw one channel. Zoomed out: per pixel the peak (light) and the mean
 * (solid). Zoomed in: second by second. Gaps where nothing was recorded. */
function trace(ctx, lane, values, base, height, max, color, width) {
  const model = state.model;
  const view = state.view;
  const secondsPerPx = (view.to - view.from) / 1000 / width;
  const scale = (v) => base + Math.sign(height) * Math.sqrt(Math.min(v, max) / max) * Math.abs(height);
  const index = (t) => Math.floor((t - model.from) / 1000);

  if (secondsPerPx <= 1) {
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.6;
    ctx.lineJoin = "round";
    ctx.beginPath();
    let open = false;
    const s0 = Math.max(0, index(view.from) - 1);
    const s1 = Math.min(model.seconds - 1, index(view.to) + 1);
    for (let s = s0; s <= s1; s++) {
      if (!lane.has[s]) {
        open = false;
        continue;
      }
      const t = model.from + s * 1000;
      const px1 = xOf(t, view, width);
      const px2 = xOf(t + 1000, view, width);
      const py = scale(values[s]);
      if (!open) {
        ctx.moveTo(px1, base);
        open = true;
      }
      // Steps: a second is a flat top.
      ctx.lineTo(px1, py);
      ctx.lineTo(px2, py);
    }
    ctx.stroke();
    return;
  }

  const peak = new Float32Array(width);
  const mean = new Float32Array(width);
  const has = new Uint8Array(width);
  for (let px = 0; px < width; px++) {
    const s0 = Math.max(0, index(view.from + (px / width) * (view.to - view.from)));
    const s1 = Math.min(model.seconds, index(view.from + ((px + 1) / width) * (view.to - view.from)) + 1);
    let n = 0;
    let sum = 0;
    let top = 0;
    for (let s = s0; s < s1; s++) {
      if (!lane.has[s]) continue;
      n++;
      sum += values[s];
      if (values[s] > top) top = values[s];
    }
    if (n) {
      has[px] = 1;
      peak[px] = top;
      mean[px] = sum / n;
    }
  }
  // Peak envelope.
  ctx.fillStyle = alpha(color, 0.3);
  for (let px = 0; px < width; px++) {
    if (!has[px]) continue;
    const y = scale(peak[px]);
    ctx.fillRect(px, Math.min(y, base), 1, Math.abs(y - base) || 0.5);
  }
  // Mean line.
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.4;
  ctx.beginPath();
  let open = false;
  for (let px = 0; px < width; px++) {
    if (!has[px]) {
      open = false;
      continue;
    }
    const y = scale(mean[px]);
    if (open) ctx.lineTo(px + 0.5, y);
    else ctx.moveTo(px + 0.5, y);
    open = true;
  }
  ctx.stroke();
}

function drawFlags(ctx, lay, width, x, c) {
  const flags = state.data.flags;
  flags.forEach((f, i) => {
    const row = lay.packed.rows[i];
    const y = 8 + row * FLAG_ROW + 3;
    let x1 = x(f.from_ms);
    let x2 = x(f.to_ms);
    if (x2 < 0 || x1 > width) return;
    x1 = Math.max(1, x1);
    x2 = Math.min(width - 1, Math.max(x2, x1 + 3));
    const color = f.severity === "high" ? c.alert : f.severity === "medium" ? c.amber : c.muted;
    const on = state.selected === i;
    ctx.strokeStyle = color;
    ctx.lineWidth = on ? 3 : 1.8;
    ctx.beginPath();
    ctx.moveTo(x1, y + 5);
    ctx.lineTo(x1, y);
    ctx.lineTo(x2, y);
    ctx.lineTo(x2, y + 5);
    ctx.stroke();
  });
}

const STEPS = [1, 5, 10, 30, 60, 300, 600, 1800, 3600, 3 * 3600, 6 * 3600, 12 * 3600, 86400].map((s) => s * 1000);

function drawAxis(ctx, y, width, c) {
  const view = state.view;
  const span = view.to - view.from;
  const step = STEPS.find((s) => (s / span) * width >= 92) || STEPS[STEPS.length - 1];
  // Ticks on local-time boundaries.
  const offset = new Date(view.from).getTimezoneOffset() * MINUTE;
  let t = Math.ceil((view.from - offset) / step) * step + offset;
  ctx.font = "12px Fredoka, system-ui, sans-serif";
  ctx.textBaseline = "top";
  ctx.fillStyle = c.muted;
  ctx.strokeStyle = alpha(c.text, 0.18);
  ctx.lineWidth = 1;
  for (; t <= view.to; t += step) {
    const px = Math.round(xOf(t, view, width)) + 0.5;
    ctx.beginPath();
    ctx.moveTo(px, y);
    ctx.lineTo(px, y + 5);
    ctx.stroke();
    const d = new Date(t);
    const midnight = d.getHours() === 0 && d.getMinutes() === 0 && d.getSeconds() === 0;
    const label = midnight && step >= MINUTE ? day(t) : clock(t, step < MINUTE);
    const w = ctx.measureText(label).width;
    ctx.fillText(label, Math.min(width - w, Math.max(0, px - w / 2)), y + 7);
  }
}

function osName(os) {
  return { macos: "macOS", windows: "Windows", linux: "Linux", other: "Other" }[os] || "Device";
}

// The whole range in a thin strip below, with the current view on it.
function drawOverview() {
  const canvas = $("overview");
  const width = canvas.parentElement.clientWidth;
  const { ctx } = fit(canvas, 40);
  const c = colors();
  const ext = state.model.extent;
  const model = state.model;
  const peak = new Float32Array(width);
  const per = (ext.to - ext.from) / width;
  for (const lane of model.lanes) {
    for (let px = 0; px < width; px++) {
      const s0 = Math.max(0, Math.floor((ext.from + px * per - model.from) / 1000));
      const s1 = Math.min(model.seconds, Math.floor((ext.from + (px + 1) * per - model.from) / 1000) + 1);
      let top = 0;
      for (let s = s0; s < s1; s++) if (lane.keys[s] > top) top = lane.keys[s];
      if (top > peak[px]) peak[px] = top;
    }
  }
  ctx.fillStyle = c.bubble;
  ctx.fillRect(0, 0, width, 40);
  ctx.fillStyle = alpha(c.pink, 0.75);
  for (let px = 0; px < width; px++) {
    if (!peak[px]) continue;
    const h = Math.sqrt(Math.min(peak[px], KEY_MAX) / KEY_MAX) * 32;
    ctx.fillRect(px, 38 - h, 1, h);
  }
  for (const f of state.data.flags) {
    if (f.severity !== "high") continue;
    const x1 = ((f.from_ms - ext.from) / (ext.to - ext.from)) * width;
    const x2 = ((f.to_ms - ext.from) / (ext.to - ext.from)) * width;
    ctx.fillStyle = c.alert;
    ctx.fillRect(x1, 0, Math.max(2, x2 - x1), 3);
  }
  const vx1 = ((state.view.from - ext.from) / (ext.to - ext.from)) * width;
  const vx2 = ((state.view.to - ext.from) / (ext.to - ext.from)) * width;
  ctx.fillStyle = alpha(c.bg, 0.55);
  ctx.fillRect(0, 0, Math.max(0, vx1), 40);
  ctx.fillRect(vx2, 0, Math.max(0, width - vx2), 40);
  ctx.strokeStyle = c.pink;
  ctx.lineWidth = 2;
  ctx.strokeRect(Math.max(1, vx1), 1, Math.max(2, Math.min(width - 1, vx2) - Math.max(1, vx1)), 38);
}

// ---------------------------------------------------------------------------
// Zooming and hovering

function setView(from, to) {
  const ext = state.model.extent;
  const fullFrom = Math.min(ext.from, state.model.from);
  const fullTo = Math.max(ext.to, state.model.to);
  let span = Math.max(20000, Math.min(to - from, fullTo - fullFrom));
  let f = from;
  if (f < fullFrom) f = fullFrom;
  if (f + span > fullTo) f = fullTo - span;
  state.view = { from: f, to: f + span };
  drawAll();
}

function resetView() {
  setView(state.model.extent.from, state.model.extent.to);
}

function stripTime(px) {
  const width = $("strip").clientWidth;
  return state.view.from + (px / width) * (state.view.to - state.view.from);
}

function localX(event, canvas) {
  return event.clientX - canvas.getBoundingClientRect().left;
}

function setupStrip() {
  const canvas = $("strip");
  const pointers = new Map();
  let pinch = null;
  let pan = null;

  canvas.addEventListener("pointerdown", (e) => {
    canvas.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, localX(e, canvas));
    if (pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      pinch = { dist: Math.abs(a - b), mid: stripTime((a + b) / 2), view: { ...state.view } };
      state.brush = null;
      pan = null;
    } else if (e.pointerType === "mouse") {
      state.brush = { x0: localX(e, canvas), x1: localX(e, canvas) };
    } else {
      pan = { x: localX(e, canvas), view: { ...state.view } };
    }
  });
  canvas.addEventListener("pointermove", (e) => {
    const px = localX(e, canvas);
    if (pointers.has(e.pointerId)) pointers.set(e.pointerId, px);
    if (pinch && pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      const factor = pinch.dist / Math.max(10, Math.abs(a - b));
      const span = (pinch.view.to - pinch.view.from) * factor;
      const share = ((a + b) / 2) / canvas.clientWidth;
      setView(pinch.mid - share * span, pinch.mid + (1 - share) * span);
      return;
    }
    if (pan) {
      const dt = ((px - pan.x) / canvas.clientWidth) * (pan.view.to - pan.view.from);
      setView(pan.view.from - dt, pan.view.to - dt);
      return;
    }
    if (state.brush) {
      state.brush.x1 = px;
      drawStrip();
      return;
    }
    hover(e);
  });
  const end = (e) => {
    pointers.delete(e.pointerId);
    if (pointers.size < 2) pinch = null;
    if (pointers.size === 0) pan = null;
    if (state.brush) {
      const { x0, x1 } = state.brush;
      state.brush = null;
      if (Math.abs(x1 - x0) > 6) {
        const a = stripTime(Math.min(x0, x1));
        const b = stripTime(Math.max(x0, x1));
        setView(a, b);
      } else {
        drawStrip();
      }
    }
  };
  canvas.addEventListener("pointerup", end);
  canvas.addEventListener("pointercancel", end);
  canvas.addEventListener("pointerleave", () => {
    state.hover = null;
    $("tip").hidden = true;
    if (!state.brush) drawStrip();
  });
  canvas.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const px = localX(e, canvas);
      const at = stripTime(px);
      const span = state.view.to - state.view.from;
      if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
        const dt = (e.deltaX / canvas.clientWidth) * span;
        setView(state.view.from + dt, state.view.to + dt);
        return;
      }
      const next = span * Math.pow(1.0018, e.deltaY);
      const share = px / canvas.clientWidth;
      setView(at - share * next, at + (1 - share) * next);
    },
    { passive: false },
  );
  canvas.addEventListener("dblclick", resetView);

  // The overview: drag the window, or click to centre it there.
  const over = $("overview");
  let dragging = null;
  const overTime = (e) => {
    const ext = state.model.extent;
    return ext.from + (localX(e, over) / over.clientWidth) * (ext.to - ext.from);
  };
  over.addEventListener("pointerdown", (e) => {
    over.setPointerCapture(e.pointerId);
    const t = overTime(e);
    const span = state.view.to - state.view.from;
    if (t < state.view.from || t > state.view.to) setView(t - span / 2, t + span / 2);
    dragging = { t, view: { ...state.view } };
  });
  over.addEventListener("pointermove", (e) => {
    if (!dragging) return;
    const dt = overTime(e) - dragging.t;
    setView(dragging.view.from + dt, dragging.view.to + dt);
  });
  const stop = () => (dragging = null);
  over.addEventListener("pointerup", stop);
  over.addEventListener("pointercancel", stop);
  over.addEventListener("dblclick", resetView);
}

function hover(e) {
  const canvas = $("strip");
  const px = localX(e, canvas);
  const py = e.clientY - canvas.getBoundingClientRect().top;
  const lay = layout(canvas.clientWidth);
  const laneIndex = Math.floor((py - lay.top) / LANE);
  const lane = state.model.lanes[laneIndex];
  state.hover = { x: px };
  const tip = $("tip");
  if (!lane) {
    tip.hidden = true;
    drawStrip();
    return;
  }
  const t = stripTime(px);
  const minute = Math.floor(t / MINUTE) * MINUTE;
  const m = lane.minutes.get(minute);
  drawStrip();
  if (!m) {
    tip.hidden = true;
    return;
  }
  fillTip(tip, lane, m, t);
  tip.hidden = false;
  const wrap = $("strip-wrap");
  const left = px + 16 + tip.offsetWidth > wrap.clientWidth ? px - 16 - tip.offsetWidth : px + 16;
  tip.style.left = `${Math.max(0, left)}px`;
  tip.style.top = `${Math.max(0, lay.top + laneIndex * LANE - 10)}px`;
}

function fillTip(tip, lane, m, t) {
  tip.replaceChildren();
  const h = m.hbs[0];
  tip.append(el("div", "t", h.session_name || "Untitled session"));
  tip.append(el("div", "s", `${h.project || "no project"}, ${h.agent}, ${clock(m.minute)}`));

  const slots = el("div", "slots");
  for (const s of h.slots || []) {
    const cell = el("span", s.focus === "agent" ? "agent" : "", s.focus);
    slots.append(cell);
  }
  tip.append(slots);

  const sec = Math.floor((t - m.minute) / 1000);
  const keys = h.keys ? h.keys.reduce((a, b) => a + b, 0) : (h.slots || []).reduce((a, s) => a + s.keys, 0);
  const mouse = h.mouse ? h.mouse.reduce((a, b) => a + b, 0) : (h.slots || []).reduce((a, s) => a + s.mouse, 0);
  const row = (label, value, warn) => {
    const r = el("div", `row${warn ? " warn" : ""}`);
    r.append(el("span", "", label), el("b", "", value));
    tip.append(r);
  };
  row("Keys and mouse this minute", `${number(keys)} / ${number(mouse)}`);
  if (h.keys) row(`At :${String(sec).padStart(2, "0")}`, `${h.keys[sec] || 0} keys, ${h.mouse[sec] || 0} mouse`);
  row("Read as", h.input_kinds || "unknown");
  const injected = h.injected ? h.injected.keys + h.injected.mouse : 0;
  if (injected) row("From software", number(injected), true);
  const p = h.prompts;
  if (p.submitted) {
    row("Prompts", `${p.submitted}${p.scripted ? `, ${p.scripted} scripted` : ""}`, p.scripted > 0);
    row("Typed and pasted", `${number(p.typed_chars)} / ${number(p.pasted_chars)} chars`, p.typed_chars > keys * 1.5 + 10);
  }
  if (h.responses && h.responses.finished) row("Responses", `${h.responses.finished}, ${number(h.responses.chars)} chars`);
  if (h.models && h.models.length) row(h.models.length > 1 ? "Models" : "Model", h.models.join(", "));
  if (h.tools && h.tools.length) {
    const calls = h.tools.filter((x) => x.calls).map((x) => `${x.calls} ${x.kind}`);
    const ms = h.tools.reduce((n, x) => n + x.ms, 0);
    row("Tool calls", `${calls.join(", ") || "none started"}${ms ? `, ${duration(ms)}` : ""}`);
  }
  const files = m.hbs.reduce((n, x) => n + (x.files ? x.files.length : 0), 0);
  if (files) row("Files changed", String(files));
  row("Credit", m.credit ? `${duration(m.credit)} (attention ${Math.round(m.multiplier * 100)}%)` : "none");
  if (m.hbs.length > 1) row("Sessions this minute", String(m.hbs.length));
  row("Client", h.client || `not reported (scheme v${h.version})`);
  if (m.stale) row("Stale", "earned nothing", true);
  else if (m.offline) row("Arrived late", duration(h.received_ms - h.minute_ms), true);
}

// ---------------------------------------------------------------------------
// Rhythm fingerprint

function drawPoincare() {
  const canvas = $("poincare");
  const { ctx, width, height } = fit(canvas, Math.min(460, canvas.clientWidth));
  const c = colors();
  const channel = state.channel;
  const max = channel === "keys" ? 24 : 60;
  const bins = channel === "keys" ? 25 : 30;
  const binOf = (v) => Math.min(bins - 1, Math.floor((Math.min(v, max) / max) * (bins - 1) + 0.0001));
  const grid = new Uint32Array(bins * bins);
  const model = state.model;
  const s0 = Math.max(0, Math.floor((state.view.from - model.from) / 1000));
  const s1 = Math.min(model.seconds - 1, Math.ceil((state.view.to - model.from) / 1000));
  let n = 0;
  let sum = 0;
  let sumSq = 0;
  let pairs = 0;
  for (const lane of model.lanes) {
    const values = lane[channel];
    for (let s = s0; s < s1; s++) {
      if (lane.has[s] !== 1) continue;
      const a = values[s];
      if (a > 0) {
        n++;
        sum += a;
        sumSq += a * a;
      }
      if (lane.has[s + 1] !== 1) continue;
      const b = values[s + 1];
      if (a === 0 && b === 0) continue;
      grid[binOf(a) * bins + binOf(b)]++;
      pairs++;
    }
  }

  const pad = { l: 34, b: 30, t: 8, r: 8 };
  const w = width - pad.l - pad.r;
  const h = height - pad.t - pad.b;
  const cell = Math.min(w, h) / bins;
  const ox = pad.l + (w - bins * cell) / 2;
  const oy = pad.t + bins * cell;

  ctx.fillStyle = c.bubble;
  ctx.fillRect(ox, pad.t, bins * cell, bins * cell);
  // The diagonal: same count two seconds running.
  ctx.strokeStyle = alpha(c.text, 0.18);
  ctx.setLineDash([4, 4]);
  ctx.beginPath();
  ctx.moveTo(ox, oy);
  ctx.lineTo(ox + bins * cell, pad.t);
  ctx.stroke();
  ctx.setLineDash([]);

  const top = grid.reduce((a, b) => Math.max(a, b), 0);
  const color = channel === "keys" ? c.pink : c.mouse;
  for (let i = 0; i < bins; i++) {
    for (let j = 0; j < bins; j++) {
      const count = grid[i * bins + j];
      if (!count) continue;
      ctx.fillStyle = alpha(color, 0.12 + 0.88 * (Math.log1p(count) / Math.log1p(top)));
      ctx.fillRect(ox + i * cell + 0.5, oy - (j + 1) * cell + 0.5, cell - 1, cell - 1);
    }
  }

  // Axes.
  ctx.fillStyle = c.muted;
  ctx.font = "12px Fredoka, system-ui, sans-serif";
  ctx.textBaseline = "top";
  const unit = channel === "keys" ? "keys" : "mouse events";
  for (const v of channel === "keys" ? [0, 6, 12, 18, 24] : [0, 15, 30, 45, 60]) {
    const p = (v / max) * (bins - 1) * cell + cell / 2;
    ctx.fillText(String(v), ox + p - 4, oy + 4);
    ctx.fillText(String(v), ox - 20, oy - p - 6);
  }
  ctx.fillText(`${unit} this second`, ox + (bins * cell) / 2 - 50, oy + 16);
  ctx.save();
  ctx.translate(ox - 44, pad.t + (bins * cell) / 2);
  ctx.rotate(-Math.PI / 2);
  ctx.textAlign = "center";
  ctx.fillText("the next second", 0, 0);
  ctx.restore();

  const note = $("cv");
  note.replaceChildren();
  if (n < 10) {
    note.append(el("span", "muted", `Too few ${unit} in view to tell.`));
    return;
  }
  const mean = sum / n;
  const cv = Math.sqrt(Math.max(0, sumSq / n - mean * mean)) / mean;
  const machine = cv < 0.2 && n >= 60;
  note.append(
    el("span", "", "Variation "),
    el(machine ? "span" : "strong", machine ? "bad" : "", `${Math.round(cv * 100)}%`),
    el("span", "muted", ` over ${number(n)} active seconds, ${Math.round(mean * 10) / 10} on average. People: about 30 to 80%.`),
  );
}

// ---------------------------------------------------------------------------
// Reading clock

function readingPoints() {
  // Per session: each prompt after a finished response, with no prompt in
  // between.
  const bySession = new Map();
  for (const lane of state.model.lanes) {
    for (const p of lane.prompts) {
      if (p.t < state.view.from || p.t > state.view.to) continue;
      push(bySession, p.hb.session, { t: p.t, prompt: true });
    }
    for (const r of lane.responses) push(bySession, r.hb.session, { t: r.t, chars: r.chars });
  }
  const points = [];
  for (const events of bySession.values()) {
    events.sort((a, b) => a.t - b.t || (a.prompt ? 1 : -1));
    let last = null;
    for (const e of events) {
      if (e.prompt) {
        if (last && e.t >= last.t) points.push({ chars: last.chars, wait: Math.max(0.5, (e.t - last.t) / 1000) });
        last = null;
      } else {
        last = e;
      }
    }
  }
  return points;
}

function push(map, key, value) {
  if (!map.has(key)) map.set(key, []);
  map.get(key).push(value);
}

function drawReading() {
  const canvas = $("reading");
  const { ctx, width, height } = fit(canvas, Math.min(460, canvas.clientWidth));
  const c = colors();
  const pad = { l: 44, b: 30, t: 8, r: 10 };
  const w = width - pad.l - pad.r;
  const h = height - pad.t - pad.b;
  const lx = [1, 5]; // 10 to 100,000 chars
  const ly = [Math.log10(0.5), Math.log10(3600)];
  const X = (chars) => pad.l + ((Math.log10(Math.max(10, chars)) - lx[0]) / (lx[1] - lx[0])) * w;
  const Y = (secs) => pad.t + h - ((Math.log10(Math.min(3600, Math.max(0.5, secs))) - ly[0]) / (ly[1] - ly[0])) * h;

  ctx.fillStyle = c.bubble;
  ctx.fillRect(pad.l, pad.t, w, h);
  // The human band: reading at 60 to 5 characters a second.
  ctx.fillStyle = alpha(c.pink, 0.18);
  ctx.beginPath();
  const steps = 40;
  for (let i = 0; i <= steps; i++) {
    const chars = Math.pow(10, lx[0] + ((lx[1] - lx[0]) * i) / steps);
    const p = [X(chars), Y(chars / 60)];
    if (i === 0) ctx.moveTo(...p);
    else ctx.lineTo(...p);
  }
  for (let i = steps; i >= 0; i--) {
    const chars = Math.pow(10, lx[0] + ((lx[1] - lx[0]) * i) / steps);
    ctx.lineTo(X(chars), Y(chars / 5));
  }
  ctx.closePath();
  ctx.fill();

  ctx.fillStyle = c.muted;
  ctx.font = "12px Fredoka, system-ui, sans-serif";
  ctx.textBaseline = "top";
  const ticks = [[10, "10"], [100, "100"], [1000, "1k"], [10000, "10k"], [100000, "100k"]];
  ticks.forEach(([v, label], i) => {
    ctx.textAlign = i === 0 ? "left" : i === ticks.length - 1 ? "right" : "center";
    ctx.fillText(label, X(v), pad.t + h + 4);
  });
  ctx.textAlign = "left";
  for (const [v, label] of [[1, "1s"], [10, "10s"], [60, "1m"], [600, "10m"], [3600, "1h"]]) {
    ctx.fillText(label, 6, Y(v) - 6);
  }
  ctx.fillText("response length (characters)", pad.l + w / 2 - 80, pad.t + h + 16);

  const points = readingPoints();
  let fast = 0;
  for (const p of points) {
    const tooFast = p.wait < p.chars / 60 && p.chars >= 300;
    if (tooFast) fast++;
    ctx.fillStyle = tooFast ? c.alert : alpha(c.text, 0.55);
    ctx.beginPath();
    ctx.arc(X(p.chars), Y(p.wait), tooFast ? 3.6 : 2.8, 0, Math.PI * 2);
    ctx.fill();
  }
  const note = $("reading-note");
  note.replaceChildren();
  if (!points.length) {
    note.append(el("span", "muted", "No prompt after a finished response in view."));
    return;
  }
  note.append(
    el(fast ? "span" : "strong", fast ? "bad" : "", `${fast} of ${points.length}`),
    el("span", "muted", " replies came faster than the response could be read."),
  );
}

// ---------------------------------------------------------------------------
// Summary and flags

function renderSummary() {
  const s = state.data.summary;
  const figures = $("figures");
  figures.replaceChildren();
  const add = (value, label, cls) => {
    const d = el("div", cls || "");
    d.append(el("strong", "", value), el("span", "", label));
    figures.append(d);
  };
  add(duration(s.credited_ms), "credited");
  add(duration(s.agent_ms), "agent time");
  add(number(s.heartbeats), "heartbeats");
  add(number(s.keys), "key presses");
  add(number(s.mouse), "mouse events");
  add(number(s.injected), "from software", s.injected ? "high" : "");
  add(number(s.prompts), s.scripted_prompts ? `prompts, ${number(s.scripted_prompts)} scripted` : "prompts", s.scripted_prompts ? "medium" : "");
  add(number(s.offline), "arrived late");
  add(number(s.stale), "stale", s.stale ? "medium" : "");
  add(number(s.flags.high || 0), "strong flags", s.flags.high ? "high" : "");
  add(number(s.flags.medium || 0), "flags", s.flags.medium ? "medium" : "");
  add(number(s.flags.low || 0), "weak flags");
}

function renderFlags() {
  const list = $("flags");
  list.replaceChildren();
  const flags = state.data.flags
    .map((f, i) => ({ f, i }))
    .sort((a, b) => (SEVERITY[a.f.severity] ?? 3) - (SEVERITY[b.f.severity] ?? 3) || a.f.from_ms - b.f.from_ms);
  if (!flags.length) {
    list.append(el("li", "empty", "Nothing flagged in this range."));
    return;
  }
  for (const { f, i } of flags) {
    const li = el("li");
    const b = el("button", `${f.severity}${state.selected === i ? " selected" : ""}`);
    b.type = "button";
    b.append(el("span", "title", f.count > 1 ? `${f.title}, ${f.count} times` : f.title));
    b.append(el("span", "when", `${span(f.from_ms, f.to_ms)}${f.device ? `, device ${f.device}` : ""}`));
    b.append(el("span", "detail", f.detail));
    b.addEventListener("click", () => {
      state.selected = state.selected === i ? null : i;
      if (state.selected !== null) {
        const pad = Math.max(60000, (f.to_ms - f.from_ms) * 0.15);
        setView(f.from_ms - pad, f.to_ms + pad);
        $("strip-wrap").scrollIntoView({ behavior: "smooth", block: "center" });
      } else {
        drawAll();
      }
      renderFlags();
    });
    li.append(b);
    list.append(li);
  }
}

function drawAll() {
  if (!state.model) return;
  drawStrip();
  drawPoincare();
  drawReading();
}

// ---------------------------------------------------------------------------
// Choosing a user and a range; the URL hash keeps both

function now() {
  return DEMO ? demoNow() : Date.now();
}

function readHash() {
  const params = new URLSearchParams(location.hash.slice(1));
  const user = params.get("user");
  const from = Number(params.get("from"));
  const to = Number(params.get("to"));
  return { user, from: from || null, to: to || null };
}

function writeHash() {
  const params = new URLSearchParams();
  if (state.userId !== null) params.set("user", state.userId);
  params.set("from", state.from);
  params.set("to", state.to);
  history.replaceState(null, "", `#${params}`);
}

function setRange(kind) {
  const t = now();
  const hours = { "24h": 24, "3d": 72, "7d": 168 }[kind] || 24;
  state.to = Math.ceil(t / MINUTE) * MINUTE;
  state.from = state.to - hours * 3600000;
  markRange(kind);
  $("day").value = "";
}

function markRange(kind) {
  for (const b of document.querySelectorAll("[data-range]")) b.classList.toggle("on", b.dataset.range === kind);
}

async function load() {
  if (state.userId === null) {
    $("status").textContent = "Find a user to review.";
    $("report").hidden = true;
    return;
  }
  writeHash();
  $("status").textContent = "Loading…";
  try {
    const data = await loadTimeline(state.userId, state.from, state.to);
    state.data = data;
    state.model = buildModel(data);
    state.selected = null;
    state.view = { ...state.model.extent };
    $("report").hidden = false;
    $("who-name").textContent = data.user.name || `User ${data.user.id}`;
    $("who-sub").textContent = `${data.user.slack_id ? `Slack ${data.user.slack_id}, ` : ""}${span(data.from, data.to)}`;
    $("status").textContent = data.heartbeats.length ? "" : "No heartbeats in this range.";
    renderSummary();
    renderFlags();
    drawAll();
  } catch (error) {
    if (error.message === "logged out" || error.message === "forbidden") return gate(error.message);
    $("status").textContent = `Couldn't load: ${error.message}`;
  }
}

function showResults(users) {
  const list = $("results");
  list.replaceChildren();
  if (!users.length) {
    list.append(el("li", "none", "Nobody found."));
  }
  for (const u of users) {
    const li = el("li");
    const b = el("button");
    b.type = "button";
    b.append(el("span", "name", u.name || `User ${u.id}`));
    const devices = `${u.devices} device${u.devices === 1 ? "" : "s"}`;
    b.append(el("span", "meta", `${u.slack_id || "no Slack ID"}, ${devices}, ${duration(u.credited_7d_ms || 0)} in 7 days`));
    b.addEventListener("click", () => pick(u));
    li.append(b);
    list.append(li);
  }
  list.hidden = false;
}

function pick(user) {
  state.userId = user.id;
  $("results").hidden = true;
  $("search").value = user.name || "";
  load();
}

function setupControls() {
  const search = $("search");
  let timer = null;
  let latest = 0;
  const run = async () => {
    const id = ++latest;
    try {
      const users = await searchUsers(search.value);
      if (id === latest) showResults(users);
    } catch (error) {
      if (error.message === "logged out" || error.message === "forbidden") gate(error.message);
    }
  };
  search.addEventListener("input", () => {
    clearTimeout(timer);
    timer = setTimeout(run, 200);
  });
  search.addEventListener("focus", run);
  search.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      const first = $("results").querySelector("button");
      if (first) first.click();
    } else if (e.key === "Escape") {
      $("results").hidden = true;
    }
  });
  document.addEventListener("click", (e) => {
    if (!e.target.closest(".search")) $("results").hidden = true;
  });

  for (const b of document.querySelectorAll("[data-range]")) {
    b.addEventListener("click", () => {
      setRange(b.dataset.range);
      load();
    });
  }
  $("day").addEventListener("change", () => {
    const value = $("day").value;
    if (!value) return;
    const [y, m, d] = value.split("-").map(Number);
    state.from = new Date(y, m - 1, d).getTime();
    state.to = new Date(y, m - 1, d + 1).getTime();
    markRange(null);
    load();
  });
  for (const b of document.querySelectorAll("[data-channel]")) {
    b.addEventListener("click", () => {
      state.channel = b.dataset.channel;
      for (const o of document.querySelectorAll("[data-channel]")) o.classList.toggle("on", o === b);
      drawPoincare();
    });
  }
  $("theme").addEventListener("click", () => {
    const root = document.documentElement;
    const current = root.dataset.theme || (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
    const next = current === "dark" ? "light" : "dark";
    root.dataset.theme = next;
    try {
      localStorage.setItem("sisyphus-theme", next);
    } catch (_) {}
    drawAll();
  });
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", drawAll);
  let resize = null;
  new ResizeObserver(() => {
    cancelAnimationFrame(resize);
    resize = requestAnimationFrame(drawAll);
  }).observe($("app"));
}

function gate(reason) {
  $("app").hidden = true;
  $("gate").hidden = false;
  $("gate-login").hidden = reason !== "logged out";
  $("gate-text").textContent =
    reason === "logged out"
      ? "Log in to review heartbeats."
      : "This page is for Sisyphus reviewers. Your account isn't one.";
}

async function start() {
  if (DEMO) {
    $("demo-note").hidden = false;
  } else {
    try {
      const me = await getJson("/api/me");
      if (!me.reviewer) return gate("forbidden");
    } catch (error) {
      return gate(error.message === "logged out" ? "logged out" : "forbidden");
    }
  }
  $("app").hidden = false;
  setupControls();
  setupStrip();

  const hash = readHash();
  if (hash.from && hash.to && hash.to > hash.from) {
    state.from = hash.from;
    state.to = hash.to;
    markRange(null);
  } else {
    setRange(DEMO ? "24h" : "24h");
  }
  if (hash.user !== null) {
    state.userId = Number(hash.user);
  } else if (DEMO && DEMO.users.length) {
    state.userId = DEMO.initial ?? DEMO.users[0].id;
  }
  if (state.userId !== null && DEMO) {
    const u = DEMO.users.find((x) => x.id === state.userId);
    if (u) $("search").value = u.name;
  }
  load();
}

start();
