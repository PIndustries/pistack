// Outside networks along the top, routers under them, then one column per
// network with the machines that have a port on it.
async function renderTopology(root, gen) {
  const tools = el("div"); tools.className = "tools";
  tools.append(el("h2", "Topology"), el("span", null, "grow"));
  const refresh = el("button", "Refresh"); refresh.type = "button";
  refresh.addEventListener("click", () => render());
  tools.append(refresh);
  root.append(tools);
  const paths = ["/network/networks", "/network/subnets", "/network/ports", "/network/routers", "/network/floating-ips", "/compute/machines"];
  let lists;
  try {
    const first = await api("GET", paths[0]);
    const rest = await Promise.all(paths.slice(1).map(p => api("GET", p).catch(() => null)));
    lists = [first].concat(rest).map(p => (p ? itemsOf(p) : []));
  } catch (e) {
    if (gen !== renderGen) return;
    setErr(e.message);
    return;
  }
  if (gen !== renderGen) return;
  const [nets, subs, ports, routers, fips, machines] = lists;
  if (!nets.length) {
    root.append(el("p", "No networks yet. Create one under Networks, give it a subnet, and it shows here.", "muted"));
    return;
  }
  const same = (a, b) => a != null && b != null && String(a) === String(b);
  const outside = nets.filter(n => n.spec && n.spec.external);
  const inner = nets.filter(n => !(n.spec && n.spec.external));
  const COL = 220, BOXW = 200, ROW = 30, HEAD = 54;
  const cols = Math.max(inner.length, routers.length, 1);
  const W = Math.max(720, cols * COL + 20);
  const colX = i => 10 + i * COL + Math.max(0, (W - 20 - cols * COL) / 2);
  const netCx = new Map();
  inner.forEach((n, i) => netCx.set(String(n.id), colX(i) + BOXW / 2));

  // Where each router sits: over the middle of the networks it joins.
  const want = routers.map((r, i) => {
    const joins = ((r.spec && r.spec.interfaces) || [])
      .map(id => subs.find(x => same(x.id, id)))
      .filter(Boolean)
      .map(x => String(x.spec && x.spec.network))
      .filter(id => netCx.has(id));
    const uniq = [...new Set(joins)];
    const cx = uniq.length ? uniq.reduce((a, id) => a + netCx.get(id), 0) / uniq.length : colX(i) + BOXW / 2;
    return { r, joins: uniq, cx };
  }).sort((a, b) => a.cx - b.cx);
  for (let i = 1; i < want.length; i++) want[i].cx = Math.max(want[i].cx, want[i - 1].cx + 170);
  const spill = want.length ? want[want.length - 1].cx + 85 - (W - 10) : 0;
  if (spill > 0) for (const w of want) w.cx -= spill;

  const topY = 10, topH = 46, rY = 104, rH = 44, nY = 196;
  const rowsOf = n => ports.filter(p => same(p.spec && p.spec.network, n.id) && p.spec.device && !routers.some(r => same(r.id, p.spec.device)));
  const tallest = Math.max(1, ...inner.map(n => rowsOf(n).length));
  const H = nY + HEAD + tallest * ROW + 24;
  const svg = svgEl("svg", { width: W, height: H, viewBox: "0 0 " + W + " " + H, role: "img", class: "topo" });
  svg.setAttribute("aria-label", "Networks, routers and the machines on them");
  const wires = svgEl("g", {});
  const boxes = svgEl("g", {});
  svg.append(wires, boxes);
  const hit = (g, view, after) => {
    g.setAttribute("class", "hit");
    g.setAttribute("tabindex", "0");
    g.setAttribute("role", "link");
    const open = () => { go(view); if (after) after(); };
    g.addEventListener("click", open);
    g.addEventListener("keydown", ev => { if (ev.key === "Enter") open(); });
  };

  // Outside networks.
  const span = (W - 20) / Math.max(outside.length, 1);
  const outBox = new Map();
  outside.forEach((n, i) => {
    const x = 10 + i * span, w = span - (outside.length > 1 ? 10 : 0);
    outBox.set(String(n.id), { x, w });
    const g = svgEl("g", {});
    g.append(svgEl("rect", { x, y: topY, width: w, height: topH, rx: 10, class: "outside" }));
    g.append(svgText(x + 14, topY + 20, clip(n.name, 40), "head"));
    const cidrs = subs.filter(x2 => same(x2.spec && x2.spec.network, n.id)).map(x2 => x2.spec.cidr).join("  ");
    const direct = rowsOf(n).length;
    g.append(svgText(x + 14, topY + 37, "outside network" + (cidrs ? "  " + cidrs : "") + (direct ? "  ·  " + direct + " machine ports" : ""), "dim"));
    hit(g, "networks");
    boxes.append(g);
  });
  if (!outside.length) boxes.append(svgText(14, topY + 28, "No outside network. Machines here cannot reach beyond their own networks.", "dim"));

  // Routers and their wires.
  for (const w of want) {
    const r = w.r;
    const x = w.cx - 75;
    const g = svgEl("g", {});
    g.append(svgEl("rect", { x, y: rY, width: 150, height: rH, rx: 22, class: "router" }));
    g.append(svgText(w.cx, rY + 19, clip(r.name, 20), "head", "middle"));
    g.append(svgText(w.cx, rY + 35, (r.status && r.status.external_ip) ? String(r.status.external_ip) : "router", "dim", "middle"));
    hit(g, "routers");
    boxes.append(g);
    const out = r.spec && r.spec.external_network && outBox.get(String(r.spec.external_network));
    if (out) {
      const ux = Math.min(Math.max(w.cx, out.x + 12), out.x + out.w - 12);
      wires.append(svgEl("path", { d: "M" + w.cx + " " + rY + " V" + (rY - 22) + " H" + ux + " V" + (topY + topH), class: "wire up" }));
    }
    w.joins.forEach((id, k) => {
      const nx = netCx.get(id);
      const mid = rY + rH + 14 + (k % 3) * 6;
      wires.append(svgEl("path", { d: "M" + w.cx + " " + (rY + rH) + " V" + mid + " H" + nx + " V" + nY, class: "wire" }));
    });
  }

  // One column per network.
  inner.forEach((n, i) => {
    const x = colX(i);
    const rows = rowsOf(n);
    const h = HEAD + Math.max(1, rows.length) * ROW + 8;
    boxes.append(svgEl("rect", { x, y: nY, width: BOXW, height: h, rx: 10, class: "box" }));
    const head = svgEl("g", {});
    head.append(svgEl("rect", { x, y: nY, width: BOXW, height: HEAD, rx: 10, fill: "transparent" }));
    head.append(svgText(x + 12, nY + 21, clip(n.name, 24), "head"));
    const cidrs = subs.filter(x2 => same(x2.spec && x2.spec.network, n.id)).map(x2 => x2.spec.cidr);
    head.append(svgText(x + 12, nY + 39, cidrs.length ? clip(cidrs.join("  "), 30) : "no subnet", "dim"));
    hit(head, "networks");
    boxes.append(head);
    boxes.append(svgEl("line", { x1: x, y1: nY + HEAD, x2: x + BOXW, y2: nY + HEAD, class: "wire", "stroke-opacity": 0.35 }));
    if (!rows.length) boxes.append(svgText(x + 12, nY + HEAD + 20, "nothing attached", "dim"));
    rows.forEach((p, k) => {
      const y = nY + HEAD + k * ROW + 20;
      const m = machines.find(mm => same(mm.id, p.spec.device));
      const phase = String((m && m.status && m.status.phase) || (p.status && p.status.phase) || "").toLowerCase();
      const fip = fips.find(f => same(f.spec && f.spec.port, p.id));
      const g = svgEl("g", {});
      g.append(svgEl("circle", { cx: x + 16, cy: y - 4, r: 4, class: phase === "ready" ? "ready" : phase === "error" ? "error" : "other" }));
      g.append(svgText(x + 28, y, clip(m ? m.name : (p.name || shortId(String(p.spec.device))), 13), "name"));
      const ip = ((p.spec && p.spec.fixed_ips) || [])[0];
      g.append(svgText(x + BOXW - 10, y, (fip && fip.spec && fip.spec.address ? "↗ " : "") + (ip ? String(ip) : ""), "dim", "end"));
      const tip = svgEl("title", {});
      tip.textContent = (m ? m.name : "port " + (p.name || p.id)) + (ip ? "  " + ip : "") + (fip && fip.spec && fip.spec.address ? "  floating IP " + fip.spec.address : "") + (phase ? "  " + phase : "");
      g.append(tip);
      if (m) hit(g, "machines", () => { openMachine = m.id; render(); });
      boxes.append(g);
    });
  });

  const wrap = el("div", null, "topo-wrap");
  wrap.append(svg);
  root.append(wrap);
  root.append(el("p", "A line from a router up is its way outside; lines down are the subnets it joins. ↗ marks a machine with a floating IP. Select a machine, router or network to open it.", "topo-key"));
}

