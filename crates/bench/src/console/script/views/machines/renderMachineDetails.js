  const panes = {
    details() {
      const kv = el("div", null, "kv");
      kv.append(
        kvLine("Name", machine.name),
        kvLine("ID", machine.id, true),
        kvLine("Project", shortId(machine.project), true),
        kvLine("Host", hostName),
        kvLine("Kind", spec.kind),
        kvLine("Size", spec.size),
        kvLine("Image", spec.image),
        kvLine("Created", machine.created_at ? new Date(machine.created_at).toLocaleString() : ""),
        kvLine("Restarts", st.restarts),
        kvLine("SSH keys", (spec.ssh_keys || []).length || "none"),
      );
      pane.append(kv);
      const conds = st.conditions || [];
      if (conds.length) {
        pane.append(el("h3", "Conditions"));
        const t = simpleTable([
          ["Condition", c => c.type],
          ["State", c => (c.status ? "yes" : "no")],
          ["Reason", c => c.reason],
          ["Since", c => ago(c.last_transition)],
          ["Message", c => (c.status ? "" : shortIds(c.message))],
        ], conds);
        t.className = "wrap";
        const wrap = el("div", null, "panel");
        wrap.append(t);
        pane.append(wrap);
      }
    },
