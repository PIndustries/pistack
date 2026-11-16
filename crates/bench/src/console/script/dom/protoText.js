function protoText(p) {
  if (p == null || p === "") return "";
  if (typeof p === "string") return p;
  if (p.type) return String(p.type);
  return "";
}
