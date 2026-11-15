async function renderBalancers(root, gen) {
  let lbs = [];
  let listeners = [];
  let pools = [];
  const problems = [];
  try { lbs = itemsOf(await api("GET", "/balancers/balancers")); }
  catch (e) { problems.push(e.message); }
  if (gen !== renderGen) return;
  try { listeners = itemsOf(await api("GET", "/balancers/listeners")); }
  catch (e) { problems.push(e.message); }
  if (gen !== renderGen) return;
  try { pools = itemsOf(await api("GET", "/balancers/pools")); }
  catch (e) { problems.push(e.message); }
  if (gen !== renderGen) return;
  if (problems.length) setErr(problems.join(" "));
  root.append(el("h3", "Balancers"));
  root.append(simpleTable([
    ["Name", i => i.name],
    ["Phase", i => i.status && i.status.phase],
    ["VIP", i => (i.status && i.status.vip) || (i.spec && i.spec.vip_address) || ""],
    ["Network", i => i.spec && i.spec.vip_network],
  ], lbs));
  root.append(el("h3", "Listeners"));
  root.append(simpleTable([
    ["Name", i => i.name],
    ["Phase", i => i.status && i.status.phase],
    ["Protocol", i => protoText(i.spec && i.spec.protocol)],
    ["Port", i => i.spec && i.spec.port],
    ["Balancer", i => i.spec && i.spec.balancer],
  ], listeners));
  root.append(el("h3", "Pools"));
  root.append(simpleTable([
    ["Name", i => i.name],
    ["Phase", i => i.status && i.status.phase],
    ["Protocol", i => protoText(i.spec && i.spec.protocol)],
    ["Algorithm", i => i.spec && i.spec.algorithm],
    ["Members", i => i.status && i.status.members],
    ["Balancer", i => i.spec && i.spec.balancer],
  ], pools));
}

