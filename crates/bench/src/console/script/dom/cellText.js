// The plain text of a row's cell, after a helper has taken it over.
function cellText(tr, idx, text) {
  const cell = tr.children[idx];
  if (cell) cell.textContent = text;
}
