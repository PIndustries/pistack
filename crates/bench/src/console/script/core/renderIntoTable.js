  // These two pages are lines that open a panel, not a table of rows.
  if (current === "projects" || current === "users") {
    const lists = await loadNames(current, gen);
    if (gen !== renderGen) return;
    root.append(renderLines(items, lists[0], lists[1], gen));
    return;
  }
  root.append(simpleTable(view.cols, items));
  const table = root.lastElementChild;
  const body = table.querySelector("tbody");
  if (current === "routers" && items.length) {
    table.querySelector("tr").append(el("th", "Subnets"));
    body.querySelectorAll("tr").forEach((tr, idx) => tr.append(routerSubnetCell(items[idx], routerSubnets)));
  }
  if (current === "hosts") hostRows(table, items);
  const canDelete = !readOnly && !view.actions && !NO_DELETE.has(current);
  const rowAction = !!(view.actions || view.apply || canDelete);
