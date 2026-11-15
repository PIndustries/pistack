  if (current === "buckets") {
    const name = el("input"); name.required = true;
    form.append(field("Bucket name", name));
    const go = el("button", "Create bucket"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      try {
        await api("POST", "/silo/buckets", { name: name.value.trim() });
        name.value = "";
        await render();
      } catch (e) { setErr(e.message); }
    });
    return form;
  }
