// Where the address points: the view, and the one row the hash names.
function hashView() {
  const parts = location.hash.replace(/^#\/?/, "").split("/");
  const view = VIEWS[parts[0]] ? parts[0] : "overview";
  let id = null;
  try { id = parts[1] ? decodeURIComponent(parts[1]) : null; } catch (e) { id = parts[1] || null; }
  if (id && view !== "hosts" && view !== "machines") id = null;
  return { view, id };
}
