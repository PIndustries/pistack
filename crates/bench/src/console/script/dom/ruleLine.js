function ruleLine(r) {
  const dir = r.direction || "—";
  const proto = r.protocol || "any";
  let ports = "any";
  if (r.port_range && r.port_range.min != null && r.port_range.max != null) {
    ports = String(r.port_range.min) === String(r.port_range.max)
      ? String(r.port_range.min)
      : String(r.port_range.min) + "-" + String(r.port_range.max);
  }
  let remote = "any";
  if (r.remote && typeof r.remote === "object") {
    if (r.remote.cidr) remote = String(r.remote.cidr);
    else if (r.remote.group) remote = String(r.remote.group);
  } else if (typeof r.remote === "string" && r.remote) {
    remote = r.remote;
  }
  return dir + " · " + proto + " · " + ports + " · " + remote;
}
