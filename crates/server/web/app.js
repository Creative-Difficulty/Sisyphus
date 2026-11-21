// Shows the login screen or, once logged in, the dashboard: a sidebar with
// Overview (tracked time and figures) and Projects (each project with its
// tracked time). Refreshed every 10 seconds. Data is only ever inserted as
// text, never as HTML.

const REFRESH_MS = 10_000;
const VIEWS = ["overview", "projects"];

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
    const [overview, projects, me] = await Promise.all([
      getJson("/api/dashboard"),
      getJson("/api/projects"),
      getJson("/api/me"),
    ]);
    showLoggedIn(true, me.name);
    renderOverview(overview);
    renderProjects(projects);
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
