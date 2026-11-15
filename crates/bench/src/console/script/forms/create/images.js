  if (current === "images") {
    const name = el("input"); name.required = true; name.placeholder = "ubuntu-24.04";
    const format = el("select");
    format.append(option("qcow2"), option("raw"), option("oci"));
    const url = el("input"); url.required = true; url.placeholder = "https://example.com/ubuntu-24.04.qcow2";
    form.append(el("strong", "Import image"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Format", format), field("URL", url));
    form.append(row);
    const go = el("button", "Import"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      go.disabled = true;
      let created = null;
      try {
        created = await api("POST", "/image/images", {
          name: name.value.trim(),
          spec: { format: format.value || "qcow2" },
        });
        await api("POST", "/image/images/" + encodeURIComponent(created.id) + "/import", { url: url.value.trim() });
        name.value = "";
        url.value = "";
        await render();
      } catch (e) {
        setErr(e.message);
        if (created) {
          name.value = "";
          url.value = "";
          await render().catch(() => {});
          setErr(e.message);
        }
      }
      go.disabled = false;
    });
    return form;
  }
