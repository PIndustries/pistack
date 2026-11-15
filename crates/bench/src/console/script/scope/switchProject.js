async function switchProject(want) {
  setErr("");
  tenantBox.disabled = true;
  projectBox.disabled = true;
  try {
    const data = await api("POST", "/identity/tokens/self/switch", { project: want });
    setToken(data.token);
    try { localStorage.setItem("pistack.project", want); } catch (e) { /* private window */ }
    // A different project is a different cloud, so what was open on the
    // old one goes with it.
    openMachine = null;
    openHost = null;
    openProject = null;
    openUser = null;
    keyOnce = null;
    sampleCache = { at: 0, rows: [] };
    for (const k of Object.keys(formOpen)) formOpen[k] = false;
    setWho(data.caller || {}, "signed in");
    loadCounts();
    await render();
  } catch (e) {
    setErr(e.message);
    loadProjects();
  }
}
