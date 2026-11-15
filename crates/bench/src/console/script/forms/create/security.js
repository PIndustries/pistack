  if (current === "security") {
    const name = el("input"); name.required = true;
    const dir = el("select");
    dir.append(option("ingress"), option("egress"));
    const proto = el("select");
    proto.append(option("", "any"), option("tcp"), option("udp"), option("icmp"));
    const port = el("input"); port.inputMode = "numeric"; port.placeholder = "22";
    const remote = el("input"); remote.placeholder = "any or 10.0.0.0/8";
    form.append(el("strong", "New security group"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Direction", dir), field("Protocol", proto), field("Port", port), field("Remote", remote));
    form.append(row);
    const go = el("button", "Create security group"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      go.disabled = true;
      try {
        const spec = {};
        const portVal = port.value.trim();
        const protoVal = proto.value;
        const remoteVal = remote.value.trim();
        const wantRule = portVal || protoVal || (remoteVal && remoteVal !== "any");
        if (wantRule) {
          const rule = { direction: dir.value || "ingress" };
          if (protoVal) rule.protocol = protoVal;
          if (portVal) {
            const n = Number(portVal);
            if (!Number.isInteger(n) || n < 1 || n > 65535) throw new Error("port must be 1-65535");
            if (rule.protocol !== "tcp" && rule.protocol !== "udp") {
              throw new Error("port range requires protocol tcp or udp");
            }
            rule.port_range = { min: n, max: n };
          }
          if (!remoteVal || remoteVal === "any") rule.remote = "any";
          else {
            rule.remote = { cidr: remoteVal };
            if (remoteVal.indexOf(":") !== -1) rule.ethertype = "ipv6";
          }
          spec.rules = [rule];
        }
        await api("POST", "/network/security-groups", { name: name.value.trim(), spec });
        name.value = ""; port.value = ""; remote.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      go.disabled = false;
    });
    const add = el("form"); add.className = "create"; add.autocomplete = "off";
    const group = el("input"); group.required = true; group.placeholder = "web";
    const ruleDir = el("select");
    ruleDir.append(option("ingress"), option("egress"));
    const ruleProto = el("select");
    ruleProto.append(option("", "any"), option("tcp"), option("udp"), option("icmp"));
    const rulePort = el("input"); rulePort.inputMode = "numeric"; rulePort.placeholder = "22 or 8000-9000";
    const ruleRemote = el("input"); ruleRemote.placeholder = "any or 10.0.0.0/8";
    add.append(el("strong", "Add rule"));
    const addRow = el("div"); addRow.className = "row";
    addRow.append(field("Group", group), field("Direction", ruleDir), field("Protocol", ruleProto), field("Port", rulePort), field("Remote", ruleRemote));
    const addGo = el("button", "Add rule"); addGo.className = "primary"; addGo.type = "submit";
    add.append(addRow, addGo);
    add.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      addGo.disabled = true;
      try {
        const sg = await api("GET", "/network/security-groups/" + encodeURIComponent(group.value.trim()));
        const rule = { direction: ruleDir.value || "ingress" };
        const protoVal = ruleProto.value;
        const portVal = rulePort.value.trim();
        const remoteVal = ruleRemote.value.trim();
        if (protoVal) rule.protocol = protoVal;
        if (portVal) {
          if (rule.protocol !== "tcp" && rule.protocol !== "udp") {
            throw new Error("port range requires protocol tcp or udp");
          }
          const bits = portVal.split("-");
          if (bits.length > 2) throw new Error("port must be 22 or 8000-9000");
          const min = Number(bits[0].trim());
          const max = bits.length === 2 ? Number(bits[1].trim()) : min;
          if (!Number.isInteger(min) || !Number.isInteger(max) || min < 1 || max > 65535 || min > max) {
            throw new Error("port must be 1-65535");
          }
          rule.port_range = { min, max };
        }
        if (!remoteVal || remoteVal === "any") rule.remote = "any";
        else {
          rule.remote = { cidr: remoteVal };
          if (remoteVal.indexOf(":") !== -1) rule.ethertype = "ipv6";
        }
        await api("POST", "/network/security-groups/" + encodeURIComponent(sg.id) + "/rules", rule);
        rulePort.value = "";
        ruleRemote.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      addGo.disabled = false;
    });
    const wrap = el("div");
    wrap.append(form, add);
    return wrap;
  }
