// Resize: change the size of a machine that is off. It stays on its host,
// so the new size only has to fit there; the server is what says whether it
// does. Nothing is offered while the machine is on, because a running
// machine cannot be resized at all.
function machineResize(machine, st, spec, mid, gen, render) {
  const wrap = el("div", null, "resizebox");
  wrap.hidden = String(st.power || "").toLowerCase() !== "off";
  const pick = el("select"); pick.title = "Change this machine's size";
  const run = el("button", "Resize"); run.type = "button";
  run.title = "Change the size. The machine stays on its host, so the host must have room.";
  wrap.append(pick, run);
  const off = String(st.power || "").toLowerCase() === "off";
  api("GET", "/compute/sizes")
    .then((page) => {
      // Filled after the answer, so a size list that is slow or missing
      // never takes the header with it.
      if (!off || gen !== renderGen) return;
      const sizes = itemsOf(page);
      pick.append(option("", "Choose a size"));
      for (const s of sizes) {
        // The size it already has is nothing to resize to.
        if (s.name === spec.size || String(s.id) === spec.size) continue;
        const r = s.spec || {};
        pick.append(option(
          s.name,
          s.name + " · " + (r.vcpus || 0) + " vCPU · " + (r.memory_mib || 0) + " MiB · " + (r.disk_gib || 0) + " GiB",
        ));
      }
      pick.disabled = pick.options.length < 2;
    })
    .catch(() => { pick.disabled = true; run.disabled = true; });
  run.disabled = true;
  pick.addEventListener("change", () => { run.disabled = !pick.value; });
  run.addEventListener("click", async () => {
    if (!pick.value) return;
    setErr("");
    run.disabled = true;
    try {
      await api("POST", "/compute/machines/" + mid + "/resize", { size: pick.value });
      await render();
    } catch (e) { run.disabled = false; setErr(e.message); }
  });
  return wrap;
}
