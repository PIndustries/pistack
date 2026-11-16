  let page = null;
  let denied = false;
  try { page = await api("GET", view.path); }
  catch (e) {
    if (gen !== renderGen) return;
    // Hosts are for cloud admins. A member gets one plain sentence rather
    // than the refusal the API sent.
    if (current === "hosts") denied = true;
    else setErr(e.message);
  }
  if (denied) {
    root.replaceChildren();
    root.append(el("p", "Hosts are visible to administrators only.", "sub"));
    return;
  }
  if (gen !== renderGen) return;
  if (current === "hosts") {
    // Placed machines give the count per host. A list that cannot be read
    // leaves the column at zero rather than failing the page.
    try { hostMachines = placeCount(itemsOf(await api("GET", "/compute/machines"))); }
    catch (e) { hostMachines = new Map(); }
    if (gen !== renderGen) return;
  }
  if (current === "machines") {
    try {
      const fp = await api("GET", "/network/floating-ips");
      if (gen !== renderGen) return;
      machineFips = itemsOf(fp);
    } catch (e) {
      if (gen !== renderGen) return;
      machineFips = [];
      setErr(e.message);
    }
  }
  const items = itemsOf(page);
  let routerSubnets = [];
  if (current === "routers") {
    try { routerSubnets = itemsOf(await api("GET", "/network/subnets")); }
    catch (e) { routerSubnets = []; }
    if (gen !== renderGen) return;
  }
  setCount(current, items.length);
  chip.textContent = String(items.length);
