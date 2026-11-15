function storedGroups() {
  try { return JSON.parse(localStorage.getItem("pistack.nav") || "{}") || {}; } catch (e) { return {}; }
}
