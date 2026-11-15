function setWho(who, fallback) {
  const roles = who.roles || [];
  readOnly = roles.length > 0 && !roles.includes("admin") && !roles.includes("member");
  const node = document.getElementById("who");
  node.textContent = (who.principal_name || fallback) + (readOnly ? " · read-only" : "");
  node.title = who.project ? "Project " + who.project : "";
  loadProjects();
}

