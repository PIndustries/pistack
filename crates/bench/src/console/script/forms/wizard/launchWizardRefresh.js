  const refresh = () => {
    const lacks = missing();
    [...nav.querySelectorAll("button")].forEach((b, i) => {
      const k = steps[i].key;
      if (i === at) b.setAttribute("aria-current", "step"); else b.removeAttribute("aria-current");
      const filled = { details: !!name.value.trim(), image: !!one(state.image), size: !!one(state.size),
        networks: state.networks.size > 0, groups: state.groups.size > 0, access: !!key.value.trim() }[k];
      b.classList.toggle("done", !!filled);
      b.classList.toggle("missing", lacks.includes(k) && at === steps.length - 1);
    });
    sum.replaceChildren();
    const parts = [name.value.trim() || "unnamed", one(state.image), one(state.size), [...state.networks].join(", ")].filter(Boolean);
    parts.forEach((p, i) => { if (i) sum.append(" · "); sum.append(i ? p : el("b", (howMany() > 1 ? howMany() + " × " : "") + p)); });
    prev.hidden = at === 0;
    next.hidden = at === steps.length - 1;
    launch.hidden = at !== steps.length - 1;
    launch.disabled = lacks.length > 0 || !data;
    launch.textContent = howMany() > 1 ? "Launch " + howMany() + " machines" : "Launch";
  };
