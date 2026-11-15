function applyFilter() {
  const q = document.getElementById("filter").value.trim().toLowerCase();
  for (const tr of document.querySelectorAll("#view tbody tr")) {
    if (tr.querySelector("td.empty")) continue;
    tr.hidden = !!q && !tr.textContent.toLowerCase().includes(q);
  }
  // The lines that open a panel filter the same way. The open one is never
  // hidden: a panel is part of the line it belongs to.
  for (const holder of document.querySelectorAll("#view .rowlines > div.line")) {
    const held = holder.nextElementSibling && holder.nextElementSibling.classList.contains("line-slot");
    holder.hidden = !!q && !held && !holder.textContent.toLowerCase().includes(q);
  }
}
