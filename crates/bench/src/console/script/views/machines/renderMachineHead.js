async function renderMachine(id, gen) {
  const root = document.getElementById("view");
  root.replaceChildren();
  const back = el("button", "← Machines", "crumb"); back.type = "button";
  back.addEventListener("click", () => { openMachine = null; render(); });
  root.append(back);
  let machine;
  try {
    machine = await api("GET", "/compute/machines/" + encodeURIComponent(id));
  } catch (e) {
    if (gen !== renderGen) return;
    setErr(e.message);
    return;
  }
  if (gen !== renderGen) return;
  // Everything else is extra: the page is drawn with whatever answered.
  const quiet = (p) => p.catch(() => null);
  // The log names a machine the way the request did: by name, or by id.
  const audit = (who) => api("GET", "/identity/audit?order=desc&limit=25&resource=" + encodeURIComponent(who));
  const [fipPage, volPage, hostPage, samples, auditPage, auditById] = await Promise.all([
    quiet(api("GET", "/network/floating-ips")),
    quiet(api("GET", "/volume/volumes")),
    quiet(api("GET", "/compute/hosts")),
    quiet(usageSamples()),
    quiet(audit(machine.name)),
    quiet(audit(machine.id)),
  ]);
  if (gen !== renderGen) return;
  const fips = fipPage ? itemsOf(fipPage) : [];
  const vols = (volPage ? itemsOf(volPage) : [])
    .filter(v => v.status && String(v.status.attached_to || "") === String(machine.id));
  const hosts = hostPage ? itemsOf(hostPage) : [];
  const st = machine.status || {};
  const spec = machine.spec || {};
  const host = hosts.find(h => h.id === st.host);
  const hostName = host ? host.name : shortId(st.host);
  const power = String(st.power || "").toLowerCase();
  const mid = encodeURIComponent(machine.id);

