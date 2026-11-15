// Shown in place of the lines when nothing in the project is capped.
function quotaNone() {
  const box = el("div", null, "panel");
  box.append(el("p", "No limits set for this project.", "muted"));
  return box;
}
