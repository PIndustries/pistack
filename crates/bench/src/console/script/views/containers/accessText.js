function accessText(item) {
  return ((item.spec && item.spec.access) || []).map(a => {
    const who = a.cidr || a.principal || "";
    const mode = a.mode || "";
    return mode ? who + " " + mode : who;
  }).filter(Boolean).join(", ");
}
