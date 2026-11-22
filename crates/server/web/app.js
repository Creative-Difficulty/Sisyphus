// Shows the login screen or, once logged in, the dashboard: a sidebar with
// Overview (tracked time and figures), Projects (each project with its
// tracked time), Devices (each linked device with its system and tracked
// time; removing one unlinks it), Settings and Heartbeats (a debug view of every uploaded heartbeat;
// clicking one shows it in full). Refreshed every 10 seconds. Data is only
// ever inserted as text, never as HTML.

const REFRESH_MS = 10_000;
const VIEWS = ["overview", "projects", "devices", "settings", "heartbeats"];

// "2h 05m"
function duration(ms) {
  const minutes = Math.floor(Math.max(0, ms) / 60000);
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function bubble(title, sub, right) {
  const li = el("li");
  const about = el("div", "about");
  about.append(el("div", "title", title), el("div", "sub", sub));
  li.append(about, el("span", "age", right));
  return li;
}

function setText(id, value) {
  document.getElementById(id).textContent = value;
}

function renderOverview(data) {
  setText("today", duration(data.tracked_today_ms));
  setText("week", duration(data.tracked_week_ms));
  setText("total", duration(data.tracked_total_ms));
  setText("agent-today", duration(data.agent_today_ms));
  setText("agent-week", duration(data.agent_week_ms));
  setText("agent-total", duration(data.agent_total_ms));
  setText("project-count", data.projects);
  setText("session-count", data.sessions);
  setText("device-count", data.devices);
}

function renderProjects(data) {
  const items = data.projects.map((p) =>
    bubble(p.name, `${p.sessions} session${p.sessions === 1 ? "" : "s"}`, duration(p.tracked_ms)),
  );
  const list = document.getElementById("projects");
  list.replaceChildren(...(items.length ? items : [el("li", "empty", "No projects tracked yet")]));
}

// Devices: an icon for the operating system, drawn for this site.
const SVG = "http://www.w3.org/2000/svg";
const OS = {
  windows: {
    name: "Windows",
    shapes: [
      ["rect", { x: 3, y: 3, width: 8, height: 8, rx: 1.5, fill: "currentColor" }],
      ["rect", { x: 13, y: 3, width: 8, height: 8, rx: 1.5, fill: "currentColor" }],
      ["rect", { x: 3, y: 13, width: 8, height: 8, rx: 1.5, fill: "currentColor" }],
      ["rect", { x: 13, y: 13, width: 8, height: 8, rx: 1.5, fill: "currentColor" }],
    ],
  },
  macos: {
    name: "macOS",
    shapes: [
      // An apple with a bite and a leaf.
      ["path", {
        fill: "currentColor",
        d: "M12 7.6c-1.2-.8-2.4-1.1-3.6-1-2.6.3-4.4 2.6-4.4 5.6 0 3.6 2.4 8.3 4.7 8.3 1 0 1.8-.6 3.3-.6s2.3.6 3.3.6c1.8 0 3.6-2.7 4.4-5.2-1.9-.7-3.1-2.4-3.1-4.4 0-1.6.8-3 2-3.8-.9-.6-1.9-.9-3-.8-1.3.1-2.5.6-3.6 1.3z",
      }],
      ["path", { fill: "currentColor", d: "M12.3 6.4c-.2-2.2 1.4-4.1 3.6-4.4.2 2.2-1.4 4.1-3.6 4.4z" }],
    ],
  },
  linux: {
    name: "Linux",
    shapes: [
      // A penguin: body, belly, eyes and feet.
      ["path", {
        fill: "none", stroke: "currentColor", "stroke-width": 2, "stroke-linejoin": "round",
        d: "M12 2.5c-2.6 0-4 2.1-4 4.8 0 1.6-.6 2.8-1.5 4.2C5.4 13.2 5 15 5 16.5 5 19.2 8 21 12 21s7-1.8 7-4.5c0-1.5-.4-3.3-1.5-5-.9-1.4-1.5-2.6-1.5-4.2 0-2.7-1.4-4.8-4-4.8z",
      }],
      ["ellipse", { cx: 12, cy: 15.2, rx: 3, ry: 3.8, fill: "currentColor", opacity: 0.35 }],
      ["circle", { cx: 10.6, cy: 6.8, r: 0.9, fill: "currentColor" }],
      ["circle", { cx: 13.4, cy: 6.8, r: 0.9, fill: "currentColor" }],
      ["path", { fill: "currentColor", d: "M10.6 9h2.8L12 10.6z" }],
    ],
  },
  other: {
    name: "Other system",
    shapes: [
      ["rect", { x: 3, y: 4, width: 18, height: 12, rx: 2.5, fill: "none", stroke: "currentColor", "stroke-width": 2 }],
      ["path", { d: "M9 20h6M12 16v4", fill: "none", stroke: "currentColor", "stroke-width": 2, "stroke-linecap": "round" }],
    ],
  },
};

function osIcon(os) {
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("class", "os");
  svg.setAttribute("aria-hidden", "true");
  for (const [tag, attrs] of (OS[os] || OS.other).shapes) {
    const shape = document.createElementNS(SVG, tag);
    for (const [key, value] of Object.entries(attrs)) shape.setAttribute(key, value);
    svg.append(shape);
  }
  return svg;
}

function osName(os) {
  return os ? (OS[os] || OS.other).name : "Unknown system";
}

function renderDevices(data) {
  const items = data.devices.map((d) => {
    const li = el("li");
    const about = el("div", "about");
    about.append(el("div", "title", osName(d.os)), el("div", "sub", `Last seen ${when(d.last_seen_ms)}`));
    const remove = el("button", "icon remove");
    remove.type = "button";
    remove.setAttribute("aria-label", `Remove this ${osName(d.os)} device`);
    remove.title = "Remove";
    remove.append(crossIcon());
    remove.addEventListener("click", () => askRemove(d));
    li.append(osIcon(d.os), about, el("span", "age", duration(d.tracked_ms)), remove);
    return li;
  });
  const list = document.getElementById("devices");
  list.replaceChildren(...(items.length ? items : [el("li", "empty", "No devices linked yet")]));
}

function crossIcon() {
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(SVG, "path");
  for (const [key, value] of Object.entries({
    d: "M6 6l12 12M18 6L6 18", fill: "none", stroke: "currentColor", "stroke-width": 2, "stroke-linecap": "round",
  })) path.setAttribute(key, value);
  svg.append(path);
  return svg;
}

// Removing a device unlinks it; its tracked time stays.
const removeDialog = document.getElementById("remove-dialog");
let removing = null;

function askRemove(device) {
  removing = device;
  setText("remove-title", `Remove this ${osName(device.os)} device?`);
  setText("remove-status", "");
  removeDialog.showModal();
}

document.getElementById("remove-cancel").addEventListener("click", () => removeDialog.close());
removeDialog.addEventListener("click", (event) => {
  if (event.target === removeDialog) removeDialog.close();
});
document.getElementById("remove-confirm").addEventListener("click", async () => {
  if (!removing) return;
  try {
    const response = await fetch("/api/devices/unlink", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ device_id: removing.device_id }),
    });
    if (response.ok || response.status === 404) {
      removeDialog.close();
      refresh();
    } else {
      setText("remove-status", "Could not remove it. Please try again.");
    }
  } catch (_) {
    setText("remove-status", "Could not remove it. Please try again.");
  }
});

// Heartbeats: newest first, 100 per page; "Show more" loads the next page.
let heartbeats = [];
let heartbeatTotal = 0;

// "Nov 21, 12:03"
function when(ms) {
  return new Date(ms).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function heartbeatTitle(hb) {
  return (hb.body && (hb.body.session_name || hb.body.agent)) || `Heartbeat ${hb.seq}`;
}

function renderHeartbeats() {
  const items = heartbeats.map((hb) => {
    const li = el("li");
    const button = el("button", "row");
    button.type = "button";
    const about = el("div", "about");
    about.append(el("div", "title", heartbeatTitle(hb)), el("div", "sub", (hb.body && hb.body.project) || ""));
    button.append(about, el("span", "age", when(hb.claimed_ms)));
    button.addEventListener("click", () => openHeartbeat(hb));
    li.append(button);
    return li;
  });
  const list = document.getElementById("heartbeats");
  list.replaceChildren(...(items.length ? items : [el("li", "empty", "No heartbeats uploaded yet")]));
  setText("heartbeat-count", heartbeatTotal ? `${heartbeatTotal} uploaded` : "");
  document.getElementById("more").hidden = heartbeats.length >= heartbeatTotal;
}

// The database sorts keys alphabetically; show them in the scheme's order.
const KEY_ORDER = [
  "version", "minute_ms", "os", "agent", "agent_session_id", "session_name", "project",
  "agent_running", "activity", "input_kinds", "slots", "tested_programs", "running_programs", "prompts", "interrupts", "responses", "files",
  "submitted", "typed_chars", "pasted_chars", "corrections", "model", "classes", "finished", "chars",
  "input", "keys", "mouse", "focus", "path", "working_dir", "agent_edits", "added", "removed",
];

function ordered(value) {
  if (Array.isArray(value)) return value.map(ordered);
  if (!value || typeof value !== "object") return value;
  const rank = (key) => (KEY_ORDER.includes(key) ? KEY_ORDER.indexOf(key) : KEY_ORDER.length);
  const keys = Object.keys(value).sort((a, b) => rank(a) - rank(b));
  return Object.fromEntries(keys.map((key) => [key, ordered(value[key])]));
}

function openHeartbeat(hb) {
  setText("dialog-title", heartbeatTitle(hb));
  setText("dialog-sub", `Minute ${when(hb.claimed_ms)}, received ${new Date(hb.received_ms).toLocaleTimeString()}`);
  const { body, ...stored } = hb;
  setText("dialog-body", JSON.stringify({ ...stored, heartbeat: ordered(body) }, null, 2));
  document.getElementById("heartbeat-dialog").showModal();
}

async function loadHeartbeats(more) {
  const offset = more ? heartbeats.length : 0;
  const page = await getJson(`/api/heartbeats?offset=${offset}`);
  heartbeatTotal = page.total;
  heartbeats = more ? heartbeats.concat(page.heartbeats) : page.heartbeats;
  renderHeartbeats();
}

const dialog = document.getElementById("heartbeat-dialog");
document.getElementById("dialog-close").addEventListener("click", () => dialog.close());
// A click on the backdrop closes it too.
dialog.addEventListener("click", (event) => {
  if (event.target === dialog) dialog.close();
});
document.getElementById("more").addEventListener("click", () => loadHeartbeats(true).catch(() => {}));

// Settings: the time zone. Until one is saved the server uses UTC; the
// browser's own zone is suggested.
function browserZone() {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch (_) {
    return "UTC";
  }
}

function zones() {
  try {
    const list = Intl.supportedValuesOf("timeZone");
    return list.includes("UTC") ? list : ["UTC", ...list];
  } catch (_) {
    return [...new Set(["UTC", browserZone()])];
  }
}

async function loadSettings() {
  const settings = await getJson("/api/settings");
  const current = settings.timezone || browserZone();
  const list = zones();
  if (!list.includes(current)) list.unshift(current);
  const select = document.getElementById("timezone");
  select.replaceChildren(
    ...list.map((zone) => {
      const option = el("option", "", zone.replaceAll("_", " "));
      option.value = zone;
      option.selected = zone === current;
      return option;
    }),
  );
  setText("settings-status", settings.timezone ? "" : "Not saved yet, so UTC is used for now");
}

document.getElementById("settings").addEventListener("submit", async (event) => {
  event.preventDefault();
  const timezone = document.getElementById("timezone").value;
  try {
    const response = await fetch("/api/settings", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ timezone }),
    });
    setText("settings-status", response.ok ? "Saved" : "Could not save");
    if (response.ok) refresh();
  } catch (_) {
    setText("settings-status", "Could not save");
  }
});

// The view comes from the URL (#overview / #projects), so reloads keep it.
function currentView() {
  const view = location.hash.slice(1);
  return VIEWS.includes(view) ? view : "overview";
}

function showView() {
  const view = currentView();
  for (const name of VIEWS) {
    document.getElementById(`view-${name}`).hidden = name !== view;
  }
  for (const link of document.querySelectorAll(".nav")) {
    const active = link.dataset.view === view;
    link.classList.toggle("active", active);
    if (active) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  }
  if (view === "heartbeats") loadHeartbeats(false).catch(() => {});
  if (view === "settings") loadSettings().catch(() => {});
}

function showLoggedIn(loggedIn, name) {
  document.getElementById("welcome").hidden = loggedIn;
  document.getElementById("app").hidden = !loggedIn;
  document.getElementById("logout").hidden = !loggedIn;
  const account = document.getElementById("account-name");
  account.hidden = !loggedIn || !name;
  account.textContent = name || "";
}

async function getJson(url) {
  const response = await fetch(url, { credentials: "same-origin" });
  if (response.status === 401) throw new Error("logged out");
  if (!response.ok) throw new Error(`${url} answered ${response.status}`);
  return response.json();
}

async function refresh() {
  try {
    const [overview, projects, devices, me] = await Promise.all([
      getJson("/api/dashboard"),
      getJson("/api/projects"),
      getJson("/api/devices"),
      getJson("/api/me"),
    ]);
    showLoggedIn(true, me.name);
    renderOverview(overview);
    renderProjects(projects);
    renderDevices(devices);
    // Only reload the first page, so loaded pages and an open popup stay.
    if (currentView() === "heartbeats" && heartbeats.length <= 100) await loadHeartbeats(false);
  } catch (error) {
    if (error.message === "logged out") showLoggedIn(false);
    // Otherwise keep showing the last data; the next refresh tries again.
  }
}

// Light/dark switch. Without a saved choice the page follows the OS.
document.getElementById("theme").addEventListener("click", () => {
  const root = document.documentElement;
  const current =
    root.dataset.theme ||
    (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
  const next = current === "dark" ? "light" : "dark";
  root.dataset.theme = next;
  try {
    localStorage.setItem("sisyphus-theme", next);
  } catch (_) {}
});

window.addEventListener("hashchange", showView);
showView();
refresh();
setInterval(refresh, REFRESH_MS);
