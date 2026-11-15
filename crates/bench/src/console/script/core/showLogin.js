function showLogin() {
  clearToken();
  keyOnce = null;
  openMachine = null;
  openHost = null;
  openProject = null;
  openUser = null;
  document.getElementById("shell").hidden = true;
  document.getElementById("login").hidden = false;
}
