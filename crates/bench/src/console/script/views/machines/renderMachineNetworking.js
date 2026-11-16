    networking() {
      const kv = el("div", null, "kv");
      kv.append(
        kvLine("Networks", (spec.networks || []).join(", ")),
        kvLine("Private addresses", (st.addresses || []).join(", "), true),
        kvLine("Security groups", (spec.security_groups || []).join(", ") || "the project's default group"),
      );
      pane.append(kv);
      pane.append(el("h3", "Floating IPs"));
      const wrap = el("div", null, "panel");
      wrap.append(simpleTable([
        ["Address", f => f.spec && f.spec.address],
        ["Reaches", f => f.status && f.status.fixed_ip],
        ["Phase", f => f.status && f.status.phase],
      ], matchingFips(machine, fips)));
      pane.append(wrap);
    },
