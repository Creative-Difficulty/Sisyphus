// Shows the login screen or, once logged in, the user's dashboard from
// /api/dashboard, refreshed every 10 seconds. Data is only ever inserted as
// text, never as HTML.

const REFRESH_MS = 10_000;

// Compact age like the client: "now", "4m", "2h", "3d", "5w".
function age(ms, now) {
  const minutes = Math.floor(Math.max(0, now - ms) / 60000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  if (minutes < 1440) return `${Math.floor(minutes / 60)}h`;
  if (minutes < 10080) return `${Math.floor(minutes / 1440)}d`;
  return `${Math.floor(minutes / 10080)}w`;
}

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function bubble(title, sub, ageText) {
  const li = el("li");
  const about = el("div", "about");
  about.append(el("div", "title", title), el("div", "sub", sub));
  li.append(about, el("span", "age", ageText));
  return li;
}

function fill(list, items, empty) {
  list.replaceChildren(...(items.length ? items : [el("li", "empty", empty)]));
}

function render(data) {
  const now = data.server_time_ms;
  document.getElementById("device-count").textContent = data.devices.length;
  document.getElementById("heartbeat-count").textContent = data.total_heartbeats;

  fill(
    document.getElementById("recent"),
    data.recent.map((h) =>
      // `h.offline` is kept for the backend (later review), not shown here.
      bubble(h.project || h.app || "Unknown", h.app, age(h.received_ms, now)),
    ),
    "No heartbeats yet",
  );

  fill(
    document.getElementById("devices"),
    data.devices.map((d) =>
      bubble(
        `Device ${d.short_id}`,
        `${d.heartbeats} heartbeat${d.heartbeats === 1 ? "" : "s"}`,
        age(d.last_seen_ms, now),
      ),
    ),
    "No devices yet",
  );
}

function show(loggedIn, name) {
  document.getElementById("welcome").hidden = loggedIn;
  document.getElementById("dashboard").hidden = !loggedIn;
  document.getElementById("logout").hidden = !loggedIn;
  const account = document.getElementById("account-name");
  account.hidden = !loggedIn || !name;
  account.textContent = name || "";
}

async function refresh() {
  try {
    const response = await fetch("/api/dashboard", { credentials: "same-origin" });
    if (response.status === 401) {
      show(false);
      return;
    }
    if (!response.ok) return;
    const data = await response.json();
    const me = await fetch("/api/me", { credentials: "same-origin" });
    show(true, me.ok ? (await me.json()).name : null);
    render(data);
  } catch (_) {
    // Keep showing the last data; the next refresh tries again.
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

refresh();
setInterval(refresh, REFRESH_MS);
