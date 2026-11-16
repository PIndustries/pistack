function setCount(view, n) {
  counts[view] = n;
  const b = navButton(view);
  if (!b) return;
  let c = b.querySelector(".count");
  if (!c) { c = el("span", null, "count"); b.append(c); }
  c.textContent = n ? String(n) : "";
}
// One pass over every list after sign-in, a few at a time, so the
// navigation shows what the cloud holds without opening each page.
