function simpleTable(cols, items) {
  const table = el("table");
  const head = el("tr");
  for (const c of cols) head.append(el("th", c[0]));
  const thead = el("thead");
  thead.append(head);
  table.append(thead);
  const body = el("tbody");
  for (const item of items) {
    const tr = el("tr");
    for (const c of cols) {
      const value = c[1](item);
      const td = el("td", show(value));
      if (c[0] === "Phase" || c[0] === "Power") td.className = "phase " + String(value || "").toLowerCase();
      tr.append(td);
    }
    body.append(tr);
  }
  if (!items.length) {
    const tr = el("tr");
    const td = el("td", "Nothing here yet.", "empty");
    td.colSpan = cols.length;
    tr.append(td);
    body.append(tr);
  }
  table.append(body);
  return table;
}

