function lastProject() {
  try { return localStorage.getItem("pistack.project") || ""; } catch (e) { return ""; }
}
// The projects this caller can hold a token for, as two boxes: the tenant,
// then the projects in it. With one choice a box only says where you are.
