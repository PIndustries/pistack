// ---- the project you are working in ---------------------------------------

const scopeBox = document.getElementById("scope");
const tenantBox = document.getElementById("tenant");
const projectBox = document.getElementById("project");
// Set once a project has been picked for a token that came without one,
// so a refusal is shown and not tried again.
let autoPicked = false;
let myProjects = [];
// A tenant is a top-level project: the one a project sits under, or the
// project itself when it sits under nothing.
