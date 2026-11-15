document.querySelector("nav").addEventListener("click", (ev) => {
  const head = ev.target.closest(".group-head");
  if (head) {
    setGroup(head.parentNode, head.getAttribute("aria-expanded") !== "true", true);
    return;
  }
  const b = ev.target.closest("button[data-view]");
  if (b) go(b.dataset.view);
});
