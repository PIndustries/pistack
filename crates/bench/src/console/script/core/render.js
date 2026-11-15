async function render(soft) {
  const gen = ++renderGen;
  clearInterval(timer);
  const view = VIEWS[current];
  const live = document.getElementById("view");
  if (current === "machines" && openMachine) {
    setErr("");
    await renderMachine(openMachine, gen);
    return;
  }
  if (current === "hosts" && openHost) {
    setErr("");
    await renderHost(openHost, gen);
    return;
  }
  // A detail page stays on screen while its list is read again underneath:
  // `soft` swaps the nodes in, an open detail page keeps them out. The list
  // is built either way, so its nav count stays true without flashing the
  // page the user is on. An open limits or roles panel is a held-open page
  // too: it lives in the lines, so any build that replaces `#view` would
  // drop it out from under the click that opened it.
  const detail = !!(openMachine || openHost || panelHeld());
  const root = (soft || detail) ? el("div") : live;
  if (!soft && !detail) live.replaceChildren();
  setErr("");
  // `rendering` says a page is being built. Two builds at once — a timed
  // refresh arriving while the last is still reading — cancel each other
  // through renderGen, and a page that swaps itself out mid-draw loses the
  // panel the user just opened.
  rendering = true;
  try {
    await renderInto(root, view, gen);
  } finally {
    // The newest build is the one still holding the flag.
    if (gen === renderGen) rendering = false;
  }
  if (gen !== renderGen) return;
  if (root !== live && !(detail && lastErr)) {
    if (!detail) live.replaceChildren(...root.childNodes);
  }
  applyFilter();
}


