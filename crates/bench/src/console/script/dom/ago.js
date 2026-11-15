function ago(ts) {
  const t = Date.parse(ts);
  if (!t) return "";
  const s = Math.max(0, Math.round((Date.now() - t) / 1000));
  if (s < 60) return s + "s ago";
  if (s < 3600) return Math.round(s / 60) + "m ago";
  if (s < 172800) return Math.round(s / 3600) + "h ago";
  return Math.round(s / 86400) + "d ago";
}
// A date kept short: month and day when the clock is that close, else to
// the day.
function dateOf(ts) {
  const t = Date.parse(ts);
  if (!t) return "";
  const d = new Date(t);
  if (Math.abs(Date.now() - t) < 86400000) return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  return d.toLocaleDateString();
}
