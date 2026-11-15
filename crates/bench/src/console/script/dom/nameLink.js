// A name that opens a detail page, in place of the row's plain text.
function nameLink(tr, idx, text, open) {
  const cell = tr.children[idx];
  if (!cell) return;
  cell.replaceChildren();
  const btn = el("button", text || "—", "linkish"); btn.type = "button";
  btn.addEventListener("click", open);
  cell.append(btn);
}

