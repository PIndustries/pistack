  // ---- details ----
  root.append(el("h3", "Details"));
  const kv = el("div", null, "kv");
  kv.append(
    kvLine("Node", spec.node_name),
    kvLine("Zone", (host.labels && host.labels.zone) || "none"),
    kvLine("Drivers", (spec.drivers || []).join(", ") || "none"),
    kvLine("Underlay", spec.underlay_ip),
    kvLine("Last seen", st.last_seen ? new Date(st.last_seen).toLocaleString() : ""),
    kvLine("Disabled", yesNo(spec.disabled)),
    kvLine("Reason given", spec.disabled_reason),
    kvLine("Certificate expires", (st.cert_expires ? new Date(st.cert_expires).toLocaleString() : "") + (certSoon(host) ? " · within two weeks" : "")),
    kvLine("ID", host.id, true),
  );
  if (st.message) kv.append(kvLine("Message", shortIds(st.message)));
  root.append(kv);

