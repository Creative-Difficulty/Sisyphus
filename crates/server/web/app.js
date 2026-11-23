// Shows the login screen or, once logged in, the dashboard: a sidebar with
// Overview (tracked time for a date range and filters: per hour/day/week,
// and by project, language, task, agent and system; clicking a bar filters
// by it), Projects (each project with its tracked time; clicking one opens
// its stats, repository and live demo links, and their settings), Devices (each linked device with its system and tracked
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

function setText(id, value) {
  document.getElementById(id).textContent = value;
}

// Overview: the date range and filters, sent to /api/dashboard. A filter
// is either absent (everything) or a value; "" selects minutes without
// one (no file changes, no prompt yet, an unknown system).
const filters = { range: "today" };
const FILTERS = ["project", "agent", "os", "language", "task"];
const ALL = "\u0000all";

const RANGE_LABEL = {
  today: "Tracked today",
  week: "Tracked this week",
  "30d": "Tracked in the last 30 days",
  all: "Tracked in total",
};
// A filter's "everything" option.
const ANY = {
  project: "Projects",
  agent: "Agents",
  os: "Systems",
  language: "Languages",
  task: "Tasks",
};
const AGENTS = { "claude-code": "Claude Code", opencode: "opencode", codex: "Codex" };
const TASKS = {
  feature: "Features",
  bug_fix: "Bug fixes",
  error_paste: "Error pastes",
  refactor: "Refactoring",
  understand_code: "Understanding code",
  learn_concept: "Learning concepts",
  ui_styling: "UI and styling",
  tests: "Tests",
  setup_deploy: "Setup and deployment",
  docs_writing: "Docs and writing",
  planning_design: "Planning and design",
  review: "Review",
  other: "Other tasks",
};

// How a filter value is shown.
function label(kind, value) {
  switch (kind) {
    case "project": return value || "No folder";
    case "agent": return AGENTS[value] || value;
    case "os": return osName(value);
    case "language": return value || "No file changes";
    case "task": return value ? TASKS[value] || value : "No prompt yet";
  }
  return value;
}

function dashboardUrl() {
  const params = new URLSearchParams({ range: filters.range });
  for (const kind of FILTERS) {
    if (kind in filters) params.set(kind, filters[kind]);
  }
  return `/api/dashboard?${params}`;
}

function setFilter(kind, value) {
  if (value === ALL) delete filters[kind];
  else filters[kind] = value;
  loadOverview().catch(() => {});
}

for (const kind of ["range", ...FILTERS]) {
  document.getElementById(`filter-${kind}`).addEventListener("change", (event) =>
    setFilter(kind, event.target.value),
  );
}

// Each filter lists every value the user has, plus "none" where that
// exists, and the selected value even if it has none in this range.
function renderFilters(options) {
  const values = {
    project: options.projects,
    agent: options.agents,
    os: options.os,
    language: [...options.languages, ""],
    task: options.tasks,
  };
  for (const kind of FILTERS) {
    const select = document.getElementById(`filter-${kind}`);
    const list = [...new Set(values[kind])];
    if (kind in filters && !list.includes(filters[kind])) list.push(filters[kind]);
    list.sort((a, b) => (a === "") - (b === "") || label(kind, a).localeCompare(label(kind, b)));
    const all = el("option", "", ANY[kind]);
    all.value = ALL;
    select.replaceChildren(
      all,
      ...list.map((value) => {
        const option = el("option", "", label(kind, value));
        option.value = value;
        return option;
      }),
    );
    select.value = kind in filters ? filters[kind] : ALL;
    select.classList.toggle("set", kind in filters);
  }
  document.getElementById("filter-range").value = filters.range;
}

// One bar per hour (today), day, or week (all time beyond three months).
function renderTimeline(data, timeline, labelRow) {
  const fmt = (options) =>
    new Intl.DateTimeFormat(undefined, { timeZone: data.timezone, ...options });
  const name = {
    hour: fmt({ hour: "2-digit" }),
    day: data.range === "week" ? fmt({ weekday: "short" }) : fmt({ day: "numeric", month: "short" }),
    week: fmt({ day: "numeric", month: "short" }),
  }[data.unit];
  const full = data.unit === "hour" ? fmt({ hour: "2-digit", minute: "2-digit" }) : fmt({ dateStyle: "medium" });
  const max = Math.max(60000, ...data.buckets.map((b) => Math.max(b.tracked_ms, b.agent_ms)));
  // At most about 8 labels.
  const every = Math.max(1, Math.ceil(data.buckets.length / 8));
  const bars = [];
  const labels = [];
  data.buckets.forEach((b, i) => {
    const column = el("div", "column");
    column.title = `${full.format(b.start_ms)}: ${duration(b.tracked_ms)} tracked, ${duration(b.agent_ms)} agent time`;
    const agent = el("div", "agent");
    agent.style.height = `${(100 * b.agent_ms) / max}%`;
    const tracked = el("div", "tracked");
    tracked.style.height = `${(100 * b.tracked_ms) / max}%`;
    column.append(agent, tracked);
    bars.push(column);
    labels.push(el("span", "", i % every === 0 ? name.format(b.start_ms) : ""));
  });
  timeline.replaceChildren(...bars);
  timeline.setAttribute("aria-label", `Tracked time per ${data.unit}`);
  labelRow.replaceChildren(...labels);
}

// A breakdown: one bar per value, longest first; on the Overview clicking
// one filters by it. At most 8, the rest summed up below.
const SHOWN = 8;

function renderBars(kind, items, list = document.getElementById(`by-${kind}`), filtering = true) {
  const shown = items.filter((item) => item.tracked_ms >= 30000);
  if (!shown.length) {
    list.replaceChildren(el("li", "empty", "Nothing tracked"));
    return;
  }
  const max = shown[0].tracked_ms;
  const rows = shown.slice(0, SHOWN).map((item) => {
    const li = el("li");
    const button = el(filtering ? "button" : "div", "bar");
    if (filtering) {
      button.type = "button";
      button.title = kind in filters ? "Show all" : `Only ${label(kind, item.name)}`;
      button.addEventListener("click", () => setFilter(kind, kind in filters ? ALL : item.name));
    }
    const fill = el("span", "fill");
    fill.style.width = `${Math.max(4, (100 * item.tracked_ms) / max)}%`;
    button.append(el("span", "name", label(kind, item.name)), fill, el("span", "time", duration(item.tracked_ms)));
    li.append(button);
    return li;
  });
  const rest = shown.slice(SHOWN);
  if (rest.length) {
    const ms = rest.reduce((sum, item) => sum + item.tracked_ms, 0);
    rows.push(el("li", "more-items", `${rest.length} more, ${duration(ms)}`));
  }
  list.replaceChildren(...rows);
}

function renderOverview(data) {
  renderFilters(data.options);
  setText("tracked-label", RANGE_LABEL[data.range]);
  setText("tracked", duration(data.tracked_ms));
  setText("agent-time", duration(data.agent_ms));
  setText("project-count", data.projects);
  setText("session-count", data.sessions);
  setText("prompt-count", count(data.prompts));
  renderTimeline(data, document.getElementById("timeline"), document.getElementById("timeline-labels"));
  renderBars("project", data.by_project);
  renderBars("language", data.by_language);
  renderBars("task", data.by_task);
  renderBars("agent", data.by_agent);
  renderBars("os", data.by_os);
}

// Answers can arrive out of order when filters change quickly; only the
// newest request is shown.
let overviewRequest = 0;

async function loadOverview() {
  const request = ++overviewRequest;
  const data = await getJson(dashboardUrl());
  if (request === overviewRequest) renderOverview(data);
}

function renderProjects(data) {
  projects = data.projects;
  const items = data.projects.map((p) => {
    const li = el("li");
    const button = el("button", "row");
    button.type = "button";
    const about = el("div", "about");
    about.append(
      el("div", "title", p.name),
      el("div", "sub", `${sessionCount(p.sessions)}, ${plural(p.prompts, "prompt")}`),
    );
    button.append(about, el("span", "age", duration(p.tracked_ms)));
    button.addEventListener("click", () => openProject(p.name));
    li.className = "project-row";
    li.append(button);
    // Links can't sit inside the button, so they follow it.
    const repo = linkButton(p.repo_url, "repo", `Open the repository of ${p.name}`);
    const demo = linkButton(p.demo_url, "demo", `Open the live demo of ${p.name}`);
    // Always there, so the times line up across rows.
    const links = el("span", "row-links");
    links.append(...[repo, demo].filter(Boolean));
    li.append(links);
    return li;
  });
  const list = document.getElementById("projects");
  list.replaceChildren(...(items.length ? items : [el("li", "empty", "No projects tracked yet")]));
}

// A round icon link that opens `url` in a new tab; null without a link.
// Only http(s) links are stored, checked again here.
const LINK_ICONS = {
  // A git branch.
  repo: [
    ["circle", { cx: 6.5, cy: 5.5, r: 2.2 }],
    ["circle", { cx: 6.5, cy: 18.5, r: 2.2 }],
    ["circle", { cx: 17.5, cy: 7.5, r: 2.2 }],
    ["path", { d: "M6.5 7.7v8.6M17.5 9.7c0 4-4.5 3.6-9.4 7.1" }],
  ],
  // A globe.
  demo: [
    ["circle", { cx: 12, cy: 12, r: 8.5 }],
    ["path", { d: "M3.5 12h17M12 3.5c2.4 2.4 3.5 5.2 3.5 8.5s-1.1 6.1-3.5 8.5c-2.4-2.4-3.5-5.2-3.5-8.5S9.6 5.9 12 3.5z" }],
  ],
};

function linkButton(url, kind, label) {
  if (!url || !/^https?:\/\//i.test(url)) return null;
  const link = el("a", "icon link-icon");
  link.href = url;
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  link.title = kind === "repo" ? "Repository" : "Live demo";
  link.setAttribute("aria-label", label);
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  for (const [tag, attrs] of LINK_ICONS[kind]) {
    const shape = document.createElementNS(SVG, tag);
    for (const [key, value] of Object.entries({
      fill: "none", stroke: "currentColor", "stroke-width": 2, "stroke-linecap": "round", "stroke-linejoin": "round", ...attrs,
    })) shape.setAttribute(key, value);
    svg.append(shape);
  }
  link.append(svg);
  return link;
}

function plural(n, word) {
  return `${count(n)} ${word}${n === 1 ? "" : "s"}`;
}

function sessionCount(n) {
  return plural(n, "session");
}

// "1,234" in the browser's language.
function count(n) {
  return new Intl.NumberFormat().format(n);
}

// A project's most changed files: characters added and removed.
function renderFiles(files) {
  const list = document.getElementById("project-files");
  if (!files.length) {
    list.replaceChildren(el("li", "empty", "No file changes tracked"));
    return;
  }
  const max = files[0].chars;
  list.replaceChildren(
    ...files.map((file) => {
      const li = el("li");
      const row = el("div", "bar");
      row.title = file.path;
      const fill = el("span", "fill");
      fill.style.width = `${Math.max(4, (100 * file.chars) / max)}%`;
      row.append(el("span", "name", file.path), fill, el("span", "time", `${count(file.chars)} chars`));
      li.append(row);
      return li;
    }),
  );
}

// A project's popup: its links, all-time stats and the links' settings.
let projects = [];
let openProjectName = null;
const projectDialog = document.getElementById("project-dialog");

function findProject(name) {
  return projects.find((p) => p.name === name);
}

function showProjectLinks(project) {
  for (const [id, url] of [["project-repo", project.repo_url], ["project-demo", project.demo_url]]) {
    const link = document.getElementById(id);
    link.hidden = !url;
    // Only http(s) links are stored; checked again here.
    if (url && /^https?:\/\//i.test(url)) link.href = url;
    else link.removeAttribute("href");
  }
  setText(
    "project-repo-hint",
    project.repo_url && project.repo_auto ? "Found on GitHub" : "",
  );
}

async function openProject(name) {
  const project = findProject(name);
  if (!project) return;
  openProjectName = name;
  setText("project-title", name);
  setText("project-sub", `Last active ${when(project.last_active_ms)}`);
  showProjectLinks(project);
  document.getElementById("project-repo-input").value = project.repo_url || "";
  document.getElementById("project-demo-input").value = project.demo_url || "";
  setText("project-status", "");
  for (const id of ["project-timeline", "project-timeline-labels", "project-by-language", "project-by-task", "project-by-agent", "project-by-os", "project-files"]) {
    document.getElementById(id).replaceChildren();
  }
  if (!projectDialog.open) projectDialog.showModal();
  try {
    const params = new URLSearchParams({ range: "all", project: name });
    const data = await getJson(`/api/dashboard?${params}`);
    if (openProjectName !== name) return;
    setText("project-tracked", duration(data.tracked_ms));
    setText("project-agent", duration(data.agent_ms));
    setText("project-sessions", data.sessions);
    setText("project-prompts", count(data.prompts));
    renderFiles(data.files);
    renderTimeline(data, document.getElementById("project-timeline"), document.getElementById("project-timeline-labels"));
    for (const kind of ["language", "task", "agent", "os"]) {
      renderBars(kind, data[`by_${kind}`], document.getElementById(`project-by-${kind}`), false);
    }
  } catch (_) {
    setText("project-status", "Could not load the stats");
  }
}

document.getElementById("project-close").addEventListener("click", () => projectDialog.close());
projectDialog.addEventListener("click", (event) => {
  if (event.target === projectDialog) projectDialog.close();
});
projectDialog.addEventListener("close", () => {
  openProjectName = null;
});

document.getElementById("project-settings").addEventListener("submit", async (event) => {
  event.preventDefault();
  const name = openProjectName;
  if (name === null) return;
  const repo = document.getElementById("project-repo-input").value.trim();
  const demo = document.getElementById("project-demo-input").value.trim();
  try {
    const response = await fetch("/api/projects/settings", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name, repo_url: repo || null, demo_url: demo || null }),
    });
    if (response.status === 400) {
      setText("project-status", "Links must start with https:// or http://");
      return;
    }
    if (!response.ok) {
      setText("project-status", "Could not save");
      return;
    }
    setText("project-status", "Saved");
    const data = await getJson("/api/projects");
    renderProjects(data);
    const project = findProject(name);
    if (project && openProjectName === name) showProjectLinks(project);
  } catch (_) {
    setText("project-status", "Could not save");
  }
});

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

// GitHub: connecting it lets the server find projects' public
// repositories by their folder names.
async function loadGithub() {
  const github = await getJson("/api/github");
  document.getElementById("github-setting").hidden = !github.enabled;
  if (!github.enabled) return;
  const connected = Boolean(github.login);
  setText(
    "github-sub",
    connected
      ? `Connected as @${github.login}. Projects are linked to your public repositories with the same name.`
      : "Links your projects to your public repositories with the same name",
  );
  document.getElementById("github-connect").hidden = connected;
  document.getElementById("github-disconnect").hidden = !connected;
}

document.getElementById("github-disconnect").addEventListener("click", async () => {
  try {
    const response = await fetch("/api/github/disconnect", { method: "POST", credentials: "same-origin" });
    setText("github-status", response.ok ? "Disconnected. Repositories already linked stay." : "Could not disconnect");
    if (response.ok) await loadGithub();
  } catch (_) {
    setText("github-status", "Could not disconnect");
  }
});

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
  if (view === "settings") {
    loadSettings().catch(() => {});
    setText("github-status", "");
    loadGithub().catch(() => {});
  }
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
    const request = ++overviewRequest;
    const [overview, projects, devices, me] = await Promise.all([
      getJson(dashboardUrl()),
      getJson("/api/projects"),
      getJson("/api/devices"),
      getJson("/api/me"),
    ]);
    showLoggedIn(true, me.name);
    if (request === overviewRequest) renderOverview(overview);
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
