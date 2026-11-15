function quotaLine(resource, line) {
  const row = el("div", null, "quota");
  row.dataset.resource = resource;
  const used = Number(line.used) || 0;
  const limit = line.limit == null ? null : Number(line.limit);
  row.append(el("span", quotaInfo(resource)[1], "what"));
  if (limit == null) {
    row.append(el("span", null, "bar"), el("span", quotaValue(resource, used) + " used", "used"));
    return row;
  }
  // A meter turns at nineteen twentieths; a limit worth acting on is nearer.
  const bar = meter(used, limit);
  if (limit > 0 && used * 10 >= limit * 9) bar.classList.add("high", "full");
  const words = quotaValue(resource, used) + " of " + quotaValue(resource, limit);
  row.append(bar, el("span", words, used >= limit ? "used bad" : "used"));
  return row;
}
// The same lines with the limits in number boxes. An empty box means no
// limit. Only the boxes that changed go, in one write, and the panel is
// read again so it shows what the server now holds.
