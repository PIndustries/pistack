function field(label, node) {
  const wrap = el("div");
  wrap.append(el("label", label), node);
  return wrap;
}

