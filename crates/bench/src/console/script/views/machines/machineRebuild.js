// Rebuild: a fresh root disk from an image. Everything else about the
// machine stays, so the only question worth asking is the one the server
// cannot answer for the operator: are they sure about the disk.
function machineRebuild(machine, st, spec, mid, render) {
  const phase = String(st.phase || "").toLowerCase();
  const b = el("button", "Rebuild", "danger"); b.type = "button";
  b.hidden = phase !== "ready" && phase !== "stopped";
  b.title = "Replace the root disk with a fresh copy of " + (spec.image || "its image");
  b.addEventListener("click", async () => {
    if (!confirm("Rebuild " + machine.name + " from " + spec.image + "? Everything on its root disk is lost.")) return;
    setErr("");
    b.disabled = true;
    try {
      // An empty object, not a missing body: the server's JSON extractor
      // needs something to read.
      await api("POST", "/compute/machines/" + mid + "/rebuild", {});
      await render();
    } catch (e) { b.disabled = false; setErr(e.message); }
  });
  return b;
}
