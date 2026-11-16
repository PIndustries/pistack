  if (current === "stacks") {
    const name = el("input"); name.required = true; name.placeholder = "web";
    const document = el("textarea"); document.required = true;
    document.placeholder = "kind: network\nname: app\nspec: {}";
    form.append(el("strong", "New stack"));
    form.append(field("Name", name), field("Document", document));
    form.append(el("p", "Apply creates network, subnet, machine, router, pod, listener, and DNS rows from this document. Every other kind is skipped.", "sub"));
    const go = el("button", "Create stack"); go.className = "primary"; go.type = "submit";
    form.append(go);
    onCreate(form, go, async () => {
      const n = name.value.trim();
      const doc = document.value;
      if (!doc.trim()) throw new Error("document must not be empty");
      await api("POST", "/blueprint/stacks", { name: n, spec: { name: n, document: doc } });
      name.value = ""; document.value = "";
    });
    return form;
  }
