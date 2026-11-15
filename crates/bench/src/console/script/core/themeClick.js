document.getElementById("theme").addEventListener("click", () => {
  const next = document.documentElement.dataset.theme === "light" ? "dark" : "light";
  setTheme(next);
  try { localStorage.setItem("pistack.theme", next); } catch (e) { /* private window */ }
});

