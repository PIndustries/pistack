  // ---- header ----
  const head = el("div", null, "mhead");
  const who = el("div", null, "who");
  const title = el("h2", host.name);
  title.append(pill(st.phase));
  if (spec.disabled) {
    const p = pill("disabled");
    p.title = spec.disabled_reason || "disabled by an admin";
    title.append(p);
  }
  const meta = el("p", null, "meta");
  const bits = [spec.node_name, (host.labels && host.labels.zone) || "no zone",
    "last seen " + (st.last_seen ? ago(st.last_seen) : "never"),
    "created " + ago(host.created_at)].filter(Boolean);
  bits.forEach((b, i) => { if (i) meta.append(" · "); meta.append(i === 0 ? el("b", b) : b); });
  who.append(title, meta);
  if (st.message && ["error", "failed"].includes(phase)) who.append(el("p", shortIds(st.message), "meta"));
  const acts = el("div", null, "acts");
  const again = el("button", "Refresh"); again.type = "button";
  again.title = "Read the host, its labels and its machines again";
  again.addEventListener("click", () => render());
  acts.append(again);
  head.append(who, acts);
  root.append(head);

