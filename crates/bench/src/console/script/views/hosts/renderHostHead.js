async function renderHost(id, gen) {
  const root = document.getElementById("view");
  root.replaceChildren();
  const back = el("button", "← Hosts", "crumb"); back.type = "button";
  back.addEventListener("click", () => go("hosts"));
  root.append(back);
  let host;
  try {
    host = await api("GET", "/compute/hosts/" + encodeURIComponent(id));
  } catch (e) {
    if (gen !== renderGen) return;
    if (e.status === 403 || /admin/.test(e.message)) {
      root.append(el("p", "Hosts are visible to administrators only.", "sub"));
      return;
    }
    setErr(e.message);
    return;
  }
  if (gen !== renderGen) return;
  const quiet = (p) => p.catch(() => null);
  const [machinePage, samples] = await Promise.all([
    quiet(api("GET", "/compute/machines")),
    quiet(usageSamples()),
  ]);
  if (gen !== renderGen) return;
  const here = (machinePage ? itemsOf(machinePage) : []).filter(m => machineHost(m) === String(host.id));
  const spec = host.spec || {};
  const st = host.status || {};
  const hid = encodeURIComponent(host.id);
  const phase = String(st.phase || "").toLowerCase();

