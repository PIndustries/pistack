function activityFeed(records, empty) {
  if (!records.length) return el("p", empty || "Nothing recorded yet.");
  const ul = el("ul", null, "feed");
  for (const r of records) {
    const s = r.spec || {};
    const li = el("li", null, s.status >= 400 ? "bad" : "");
    const what = el("span", null, "what");
    what.append(el("b", s.principal || "someone"), " " + (s.action || String(s.method || "").toLowerCase()) +
      (s.kind ? " " + s.kind : "") + (s.resource ? " " + shortId(s.resource) : ""));
    what.title = (s.method || "") + " " + (s.path || "");
    li.append(el("span", ago(r.created_at), "when"), what, el("span", String(s.status || ""), "code"));
    ul.append(li);
  }
  return ul;
}

