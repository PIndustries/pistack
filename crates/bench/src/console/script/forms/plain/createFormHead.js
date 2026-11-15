function createForm() {
  if (current === "machines") return launchWizard();
  const form = el("form");
  form.className = "create";
  form.autocomplete = "off";
