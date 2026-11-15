document.getElementById("login-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const err = document.getElementById("login-err");
  err.textContent = "";
  try {
    const data = await api("POST", "/identity/tokens", {
      name: document.getElementById("user").value.trim(),
      password: document.getElementById("pass").value,
    }, false);
    setToken(data.token);
    document.getElementById("pass").value = "";
    setWho(data.caller || {}, "admin");
    showApp();
    const at = hashView();
    go(at.view, true, at.id);
    loadCounts();
  } catch (e) {
    err.textContent = e.message === "invalid credentials"
      ? "Password was not accepted. Clear the box and paste it again. A password manager may have replaced it."
      : e.message;
  }
});
