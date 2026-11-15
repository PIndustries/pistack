if (token()) {
  api("GET", "/identity/tokens/self").then((who) => {
    setWho(who || {}, "signed in");
    showApp();
    const at = hashView();
    go(at.view, true, at.id);
    loadCounts();
  }).catch(() => showLogin());
}
