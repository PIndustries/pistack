function clip(text, n) {
  const t = String(text == null ? "" : text);
  return t.length > n ? t.slice(0, n - 1) + "…" : t;
}
