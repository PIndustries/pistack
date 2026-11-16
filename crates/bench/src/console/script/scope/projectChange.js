projectBox.addEventListener("change", () => { if (projectBox.value) switchProject(projectBox.value); });
// Picking a tenant goes to the project used last in it, else its first.
