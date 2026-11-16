
let current = "overview";
let timer = 0;
// Row counts shown next to each navigation entry, by view name.
const counts = {};
// Which create forms are open, by view name.
const formOpen = {};
let lastErr = "";
// A page build in flight. The timed refresh will not start a second one on
// top of it.
let rendering = false;
// Is an open panel on screen right now? An answer about the page, so a
// page that is not the panel's own cannot be taken for one. A panel is
// held open by what is on screen, not only by what the state remembers:
// it belongs to the lines it was drawn into, so it is matched against
// them. The moment the view changes, whatever the route came through —
// menu, address, Back — the next build replaces the lines, the panel goes
// with them, and nothing here keeps it.
function panelHeld() {
  if (!openProject && !openUser) return false;
  return !!document.querySelector("#view .rowlines > .line-slot");
}
let readOnly = false;
let renderGen = 0;
let openMachine = null;
let openHost = null;
let machineFips = [];
// One-time key secret. Shown in the DOM, then dropped. Not stored.
let keyOnce = null;
// The project a limits panel is open under, and the user a roles panel is
// open under. Both are dropped when you leave the page.
let openProject = null;
let openUser = null;
// Project names, so a line can say what an id points at.
const projectNames = new Map();

