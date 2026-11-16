function selectorText(sel) {
  if (!sel || typeof sel !== "object") return "";
  return Object.keys(sel).map(k => k + "=" + sel[k]).join(", ");
}
