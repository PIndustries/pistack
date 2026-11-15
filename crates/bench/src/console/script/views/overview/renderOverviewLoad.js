async function renderOverview(root, gen) {
  const wanted = ["hosts", "machines", "pods", "networks", "fips", "volumes", "images", "alarms"];
  const got = {};
  const failed = [];
  await Promise.all(wanted.map(async (v) => {
    try { got[v] = itemsOf(await api("GET", VIEWS[v].path)); }
    catch (e) { got[v] = []; failed.push(VIEWS[v].title); }
  }));
  // Extras that only some callers may read. The page is whole without them.
  const quiet = (p) => p.catch(() => null);
  const here = myProjects.find(p => p.current) || myProjects[0];
  const [cluster, samples, auditPage, info, capped] = await Promise.all([
    quiet(api("GET", "/cluster")),
    quiet(usageSamples()),
    quiet(api("GET", "/identity/audit?order=desc&limit=8")),
    quiet(api("GET", "/", undefined, false)),
    here ? quiet(quotaBlock(String(here.id || here.name), true)) : Promise.resolve(null),
  ]);
  if (gen !== renderGen) return;
  for (const v of wanted) if (!failed.includes(VIEWS[v].title)) setCount(v, got[v].length);
  if (failed.length === wanted.length) { setErr("The server did not answer. Is it still running?"); return; }

  const tools = el("div", null, "tools");
  tools.append(el("h2", "Overview"), el("span", null, "grow"));
  const refresh = el("button", "Refresh"); refresh.type = "button";
  refresh.addEventListener("click", () => render());
  tools.append(refresh);
  root.append(tools);

