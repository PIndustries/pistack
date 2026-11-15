const liveBox = document.getElementById("live");
try { liveBox.checked = localStorage.getItem("pistack.live") !== "off"; } catch (e) { /* private window */ }
liveBox.addEventListener("change", () => {
  try { localStorage.setItem("pistack.live", liveBox.checked ? "on" : "off"); } catch (e) { /* private window */ }
});
// A reload must not take anything out from under the user: not a form
// they are filling in, a one-time secret, or a machine's console.
setInterval(() => {
  if (!liveBox.checked || document.hidden || !token()) return;
  if (document.getElementById("shell").hidden) return;
  // What is open stays open: a machine, a host, an open limits or roles
  // panel, a form being filled in.
  if (openMachine || openHost || openProject || openUser || formOpen[current]) return;
  // A panel is held open by what is on screen, not only by what the state
  // remembers: a click that has not finished drawing still counts.
  if (panelHeld()) return;
  // Nor over a build still in flight: a slow page (an overview reads a
  // dozen lists) would otherwise have its build cancelled by the next tick.
  if (rendering) return;
  const view = document.getElementById("view");
  if (view.contains(document.activeElement) || view.querySelector(".once")) return;
  render(true);
}, 5000);

