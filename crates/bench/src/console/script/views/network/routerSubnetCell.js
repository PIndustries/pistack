// The subnets a router joins, each with a way to take it off, and a box to
// add another. The whole list is sent each time.
function routerSubnetCell(router, subnets) {
  const td = el("td");
  const wrap = el("div", null, "taglist");
  const joined = ((router.spec && router.spec.interfaces) || []).map(String);
  const send = async (list) => {
    setErr("");
    try { await api("PUT", "/network/routers/" + router.id + "/interfaces", { subnets: list }); await render(); }
    catch (e) { setErr(e.message); }
  };
  for (const id of joined) {
    const sub = subnets.find(x => String(x.id) === id);
    const tag = el("span", sub ? sub.name + " " + ((sub.spec && sub.spec.cidr) || "") : shortId(id), "tag");
    if (!readOnly) {
      const x = el("button", "×", "x"); x.type = "button";
      x.title = "Take this subnet off the router";
      x.setAttribute("aria-label", "Detach " + (sub ? sub.name : id));
      x.addEventListener("click", () => {
        if (confirm("Detach " + (sub ? sub.name : id) + " from " + router.name + "? Machines on it lose their way out.")) send(joined.filter(j => j !== id));
      });
      tag.append(x);
    }
    wrap.append(tag);
  }
  if (!joined.length) wrap.append(el("span", "none", "muted"));
  // A router can only join a subnet that has a gateway address.
  const free = subnets.filter(x => !joined.includes(String(x.id)) && x.spec && x.spec.gateway);
  if (!readOnly && free.length) {
    const pick = el("select");
    pick.setAttribute("aria-label", "Attach a subnet to " + router.name);
    pick.append(option("", "Attach subnet…"));
    for (const x of free) pick.append(option(String(x.id), x.name + " " + ((x.spec && x.spec.cidr) || "")));
    pick.addEventListener("change", () => { if (pick.value) send(joined.concat([pick.value])); });
    wrap.append(pick);
  }
  td.append(wrap);
  return td;
}

// ---- topology --------------------------------------------------------------

