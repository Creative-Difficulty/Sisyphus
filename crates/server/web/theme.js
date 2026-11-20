// Apply a saved light/dark choice before the page paints. Loaded in <head>
// as a file because the content security policy forbids inline scripts.
try {
  const saved = localStorage.getItem("sisyphus-theme");
  if (saved === "light" || saved === "dark") document.documentElement.dataset.theme = saved;
} catch (_) {}
