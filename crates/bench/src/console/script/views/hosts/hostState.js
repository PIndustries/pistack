// ---- hosts ----------------------------------------------------------------

// Machines on each host, read when the host list is drawn.
let hostMachines = new Map();
// The drain and evacuate answers, by host id: an action redraws the page,
// and the report is the one thing that survives it.
const reportHold = new Map();
// Label rows being filled in, so a redraw neither drops what was typed nor
// closes the editor under the hand typing it.
// The label editor's state, outside the nodes: which host the draft
// belongs to, the rows as typed, and whether the editor is open. Saving or
// cancelling closes it; leaving for another host drops it.
