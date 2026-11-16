async function loadCounts() {
  const names = Object.keys(VIEWS).filter(v => VIEWS[v].path);
  const next = async () => {
    while (names.length) {
      const v = names.shift();
      try { setCount(v, itemsOf(await api("GET", VIEWS[v].path)).length); }
      catch (e) { /* a module that is off, or a list this role cannot read */ }
      if (!token()) return;
    }
  };
  await Promise.all([next(), next(), next(), next()]);
}

// ---- row filter, live reload, theme ---------------------------------------

