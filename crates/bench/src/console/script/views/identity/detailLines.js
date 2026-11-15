function detailLines(items, title, sub, open, idOf, openTo, draw, controls) {
  const lines = el("div", null, "rowlines");
  if (!items.length) {
    lines.append(el("p", "Nothing here yet.", "empty"));
    return lines;
  }
  const slots = [];
  for (const item of items) {
    const id = idOf(item);
    // A line is a div holding two siblings: the button that opens the
    // panel — name and sub text only — and its controls at the right end
    // of the row. A control inside the button would be invalid markup,
    // and a click landing on the name must reach the button, so the
    // handler does not care what the event passed through.
    const holder = el("div", null, "line");
    const btn = el("button", null, "line-open");
    btn.type = "button";
    const name = el("b");
    btn.append(name, el("span", sub(item) || ""));
    title(item, name);
    holder.append(btn);
    const acts = controls(item);
    if (acts && acts.childNodes.length) holder.append(acts);
    const slot = el("div", null, "line-slot");
    slots.push(slot);
    lines.append(holder);
    // The panel belongs to its line: it sits after the line it opened
    // from, whether it was open when the page was drawn or just clicked.
    if (String(open) === String(id)) holder.after(slot);
    // One fill at a time. A redraw replaces the slot whole, so a change in
    // the panel never stacks on the panel it came from; and a draw that
    // came from an older fill is dropped, never left to land late. The
    // draw is handed the newest build, not the one that first drew it: a
    // change made from a panel older than the page is still drawn.
    let token = 0;
    slot.fill = (...nodes) => {
      slot.replaceChildren(...nodes);
      if (slot.isConnected) applyFilter();
    };
    const fill = () => {
      const mine = ++token;
      slot.replaceChildren();
      // The sink is handed out node by node: `put(who, node)`, `put(out)`.
      const put = (...nodes) => {
        if (mine === token && slot.isConnected) slot.fill(...nodes);
      };
      // A draw that came from a build since replaced is dropped: its page
      // is gone, and putting its nodes down would bury the page standing.
      const at = renderGen;
      if (mine === token && at === renderGen) {
        Promise.resolve(draw(item, put, at)).catch((e) => { if (mine === token) setErr(e.message); });
      }
    };

    btn.setAttribute("aria-expanded", String(String(open) === String(id)));
    if (String(open) === String(id)) fill();
    btn.addEventListener("click", () => {
      const wasOpen = btn.getAttribute("aria-expanded") === "true";
      for (const other of lines.querySelectorAll("button.line-open")) other.setAttribute("aria-expanded", "false");
      for (const s of slots) if (s.isConnected) s.remove();
      if (wasOpen) { open = null; openTo(null); }
      else { open = id; openTo(id); holder.after(slot); btn.setAttribute("aria-expanded", "true"); fill(); }
      applyFilter();
    });
  }
  return lines;
}
// Projects and users are lines that open a panel, not a table of rows.
