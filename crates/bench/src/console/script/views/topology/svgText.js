function svgText(x, y, text, cls, anchor) {
  const t = svgEl("text", { x, y });
  if (cls) t.setAttribute("class", cls);
  if (anchor) t.setAttribute("text-anchor", anchor);
  t.textContent = text;
  return t;
}
