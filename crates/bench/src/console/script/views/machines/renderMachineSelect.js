  // ---- keeping it current ----
  const phase = String(st.phase || "").toLowerCase();
  if (["pending", "creating", "deleting", "updating"].includes(phase)) {
    timer = setInterval(() => {
      if (machineTab === "console" || root.contains(document.activeElement)) return;
      render();
    }, 3000);
    return;
  }
  // New points for the charts, without drawing the page again.
  const push = (list, t, v) => {
    if (v === null || (list.length && list[list.length - 1].t >= t)) return;
    list.push({ t, v });
    if (list.length > 120) list.shift();
  };
