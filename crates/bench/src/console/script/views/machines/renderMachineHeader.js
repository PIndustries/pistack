  // ---- header ----
  const head = el("div", null, "mhead");
  const who = el("div", null, "who");
  const title = el("h2", machine.name);
  title.append(pill(st.phase));
  if (power) title.append(pill(power));
  const meta = el("p", null, "meta");
  const bits = [spec.kind, spec.size, spec.image, hostName ? "on " + hostName : "", "created " + ago(machine.created_at)]
    .filter(Boolean);
  bits.forEach((b, i) => { if (i) meta.append(" · "); meta.append(i < 3 ? el("b", b) : b); });
  who.append(title, meta);
  const acts = el("div", null, "acts");
  const act = (label, cls, path, method, after) => {
    const b = el("button", label, cls || ""); b.type = "button";
    b.addEventListener("click", async () => {
      if (method === "DELETE" && !confirm("Delete " + machine.name + "? Its root disk is removed.")) return;
      setErr("");
      b.disabled = true;
      try {
        await api(method || "POST", "/compute/machines/" + mid + (path || ""));
        if (after) after();
        await render();
      } catch (e) { b.disabled = false; setErr(e.message); }
    });
    return b;
  };
  const start = act("Start", "primary", "/start");
  const stop = act("Stop", "", "/stop");
  const reboot = act("Reboot", "", "/reboot");
  const rebuild = machineRebuild(machine, st, spec, mid, render);
  const resize = machineResize(machine, st, spec, mid, gen, render);
  const del = act("Delete", "danger", "", "DELETE", () => { openMachine = null; });
  // Only what makes sense for the state it is in.
  start.hidden = power === "on";
  stop.hidden = power !== "on";
  reboot.disabled = power !== "on";
  acts.append(start, stop, reboot, rebuild, resize, del);
  head.append(who, acts);
  root.append(head);
  if (st.message && ["error", "failed"].includes(String(st.phase).toLowerCase())) {
    root.append(el("p", shortIds(st.message), "once"));
  }

