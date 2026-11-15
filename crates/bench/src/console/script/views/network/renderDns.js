async function renderDns(root, gen) {
  let zones = [];
  let records = [];
  const problems = [];
  try { zones = itemsOf(await api("GET", "/dns/zones")); }
  catch (e) { problems.push(e.message); }
  if (gen !== renderGen) return;
  try { records = itemsOf(await api("GET", "/dns/recordsets")); }
  catch (e) { problems.push(e.message); }
  if (gen !== renderGen) return;
  if (problems.length) setErr(problems.join(" "));
  root.append(el("h3", "Zones"));
  root.append(simpleTable([
    ["Name", i => i.name],
    ["Phase", i => i.status && i.status.phase],
    ["Apex", i => i.spec && i.spec.name],
  ], zones));
  root.append(el("h3", "Records"));
  root.append(simpleTable([
    ["Name", i => i.spec && i.spec.name],
    ["Type", i => i.spec && i.spec.type],
    ["Records", i => ((i.spec && i.spec.records) || []).join(", ")],
  ], records));
}

