async function renderMachine(id, gen, soft) {
  const live = document.getElementById("view");
  // A soft pass is a Live reload. It holds the page out from under a control
  // the user is on, and leaves an open console pane — and its websocket —
  // alone; a fresh draw replaces everything.
  const held = soft && (live.contains(document.activeElement) || machineHeld());
  if (soft && held) return;
  // A fresh draw builds straight into the live view, exactly as before. A soft
  // pass builds into a detached element and swaps it in only once every read
  // has answered, so the open page is never blanked while the reads are in
  // flight. `root` is where the rest of this function (and every file joined
  // after it) appends; on a soft pass it starts as that detached container.
  const root = soft ? document.createElement("div") : live;
  if (!soft) live.replaceChildren();
  if (!soft) machineTab = "details";
  const back = el("button", "← Machines", "crumb"); back.type = "button";
  back.addEventListener("click", () => { openMachine = null; render(); });
  root.append(back);
  let machine;
  try {
    machine = await api("GET", "/compute/machines/" + encodeURIComponent(id));
  } catch (e) {
    if (gen !== renderGen) return;
    // A soft pass leaves the page already on screen untouched when the machine
    // read fails — except a 404, which means the machine is gone: drop it and
    // show the list.
    if (soft) {
      if (e.status === 404) { openMachine = null; render(); }
      return;
    }
    setErr(e.message);
    return;
  }
  if (gen !== renderGen) return;
  // Everything else is extra: the page is drawn with whatever answered.
  const quiet = (p) => p.catch(() => null);
  // A soft pass keeps whatever it is holding: if the machine is gone while the
  // rest are still being read, drop it and show the list instead of building a
  // page around a machine that is not there.
  const gone = (e) => { if (soft && e && e.status === 404) { openMachine = null; render(); } return null; };
  // The log names a machine the way the request did: by name, or by id.
  const audit = (who) => api("GET", "/identity/audit?order=desc&limit=25&resource=" + encodeURIComponent(who));
  const [fipPage, volPage, hostPage, samples, auditPage, auditById] = await Promise.all([
    quiet(api("GET", "/network/floating-ips")),
    quiet(api("GET", "/volume/volumes")),
    quiet(api("GET", "/compute/hosts")),
    quiet(usageSamples()),
    quiet(audit(machine.name)).catch(gone),
    quiet(audit(machine.id)).catch(gone),
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
  // Stamp the machine on the live node. On a fresh draw `root` is the live
  // view; on a soft pass it is the detached build, so stamp the view that will
  // still be here once this page is swapped in.
  live.dataset.machineId = String(machine.id);

