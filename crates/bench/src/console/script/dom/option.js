function option(value, label) {
  const o = el("option", label == null ? value : label);
  o.value = String(value);
  return o;
}
