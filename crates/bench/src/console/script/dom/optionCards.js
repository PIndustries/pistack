// ---- launching a machine ---------------------------------------------------

// Cards to pick from. `multi` lets several be on at once.
function optionCards(items, picked, multi, onChange, emptyText) {
  const box = el("div", null, "opts");
  if (!items.length) { box.append(el("div", emptyText, "nothing")); return box; }
  for (const it of items) {
    const b = el("button"); b.type = "button";
    b.append(el("b", it.title), el("span", it.detail || " "));
    b.disabled = !!it.disabled;
    if (it.disabled && it.why) b.title = it.why;
    b.setAttribute("aria-pressed", String(picked.has(it.value)));
    b.addEventListener("click", () => {
      if (multi) { if (picked.has(it.value)) picked.delete(it.value); else picked.add(it.value); }
      else { picked.clear(); picked.add(it.value); }
      for (const o of box.children) o.setAttribute("aria-pressed", "false");
      [...box.children].forEach((o, i) => o.setAttribute("aria-pressed", String(picked.has(items[i].value))));
      onChange();
    });
    box.append(b);
  }
  return box;
}

