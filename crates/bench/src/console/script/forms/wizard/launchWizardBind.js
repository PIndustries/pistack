  steps.forEach((s, i) => {
    const b = el("button"); b.type = "button";
    b.append(el("span", String(i + 1), "n"), s.title);
    b.addEventListener("click", () => { at = i; draw(); });
    nav.append(b);
  });
  prev.addEventListener("click", () => { at = Math.max(0, at - 1); draw(); });
  next.addEventListener("click", () => { at = Math.min(steps.length - 1, at + 1); draw(); });
  name.addEventListener("input", refresh);
  count.addEventListener("input", refresh);
  key.addEventListener("input", refresh);
  // Enter in a field moves on; it does not launch from the first step.
  form.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" && ev.target.tagName === "INPUT" && at !== steps.length - 1) { ev.preventDefault(); next.click(); }
  });

  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    if (at !== steps.length - 1 || missing().length) { at = steps.length - 1; draw(); return; }
    setErr("");
    launch.disabled = true;
    const spec = { kind: "vm", size: one(state.size), image: one(state.image), networks: [...state.networks], volumes: [] };
    if (state.groups.size) spec.security_groups = [...state.groups];
    if (one(state.zone)) spec.zone = one(state.zone);
    if (userData.value.trim()) spec.user_data = userData.value;
    if (key.value.trim()) {
      spec.ssh_keys = [key.value.trim()];
      try { localStorage.setItem("pistack.sshkey", key.value.trim()); } catch (e) { /* private window */ }
    }
    const base = name.value.trim();
    const names = howMany() > 1 ? Array.from({ length: howMany() }, (_, i) => base + "-" + (i + 1)) : [base];
    const made = [];
    try {
      for (const n of names) {
        await api("POST", "/compute/machines", { name: n, spec });
        made.push(n);
      }
      formOpen[current] = false;
      await render();
    } catch (e) {
      setErr((made.length ? "Launched " + made.join(", ") + ", then: " : "") + e.message);
      launch.disabled = false;
    }
  });
