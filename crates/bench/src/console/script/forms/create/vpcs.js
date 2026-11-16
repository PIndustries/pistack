if (current === "vpcs") {
    const wrap = el("div");
    const name = el("input"); name.required = true; name.placeholder = "prod";
    const cidrs = el("input"); cidrs.required = true; cidrs.placeholder = "10.20.0.0/16";
    const network = el("input"); network.placeholder = "optional";
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Blocks", cidrs), field("External network", network));
    form.append(el("strong", "New VPC"), row);
    const go = el("button", "Create VPC"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      go.disabled = true;
      try {
        const cidrs2 = cidrs.value.split(",").map(s => s.trim()).filter(Boolean);
        if (!cidrs2.length) throw new Error("give at least one block");
        const spec = { cidrs: cidrs2 };
        const ext = network.value.trim();
        if (ext) {
          const net = await api("GET", "/network/networks/" + encodeURIComponent(ext));
          spec.external_network = net.id;
        }
        await api("POST", "/network/vpcs", { name: name.value.trim(), spec });
        name.value = ""; cidrs.value = ""; network.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      go.disabled = false;
    });
    const sub = el("form"); sub.className = "create"; sub.autocomplete = "off";
    const vpcName = el("input"); vpcName.required = true; vpcName.placeholder = "prod";
    const subName = el("input"); subName.required = true; subName.placeholder = "web";
    const block = el("input"); block.required = true; block.placeholder = "24";
    const srow = el("div"); srow.className = "row";
    srow.append(field("VPC", vpcName), field("Subnet name", subName), field("Block or prefix length", block));
    sub.append(el("strong", "Add subnet"), srow);
    const sgo = el("button", "Add subnet"); sgo.className = "primary"; sgo.type = "submit";
    sub.append(sgo);
    sub.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      sgo.disabled = true;
      try {
        const vpc = await api("GET", "/network/vpcs/" + encodeURIComponent(vpcName.value.trim()));
        await api("POST", "/network/vpcs/" + encodeURIComponent(vpc.id) + "/subnets", {
          name: subName.value.trim(),
          cidr: block.value.trim(),
        });
        vpcName.value = ""; subName.value = ""; block.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      sgo.disabled = false;
    });
    wrap.append(form, sub);
    return wrap;
  }
