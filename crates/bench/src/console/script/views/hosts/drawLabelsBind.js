  editBtn.addEventListener("click", () => {
    // This closure holds the nodes of the page on screen, so it opens in
    // place. The draft lives outside it, so the next redraw finds the
    // editor open too.
    draft.open = true;
    draw();
    const first = box.querySelector("input");
    if (first) first.focus();
  });
  box.append(shown, editBtn, lbox, foot);
  draw();
}
// What was done, newest first, from the audit log.
