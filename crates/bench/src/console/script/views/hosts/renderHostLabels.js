  // ---- labels ----
  const labelHead = el("h3", "Labels");
  if (!readOnly) {
    // The head carries the redraw: the page has no other hand control, and
    // every action on it redraws through the same door.
    labelHead.append(" ");
    const redraw = el("button", "Refresh"); redraw.type = "button";
    redraw.className = "linkish";
    redraw.title = "Read the host, its labels and its machines again";
    redraw.addEventListener("click", () => render());
    labelHead.append(redraw);
  }
  root.append(labelHead);
  const labelBox = el("div", null, "detail");
  labelBox.style.width = "100%";
  root.append(labelBox);
  drawLabels(labelBox, host);

