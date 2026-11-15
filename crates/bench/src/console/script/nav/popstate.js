window.addEventListener("popstate", () => {
  if (!token()) return;
  const at = hashView();
  go(at.view, true, at.id);
});
// The address is a way in from outside the page as much as from Back:
// typing `#/projects` in the bar, a bookmark, a link. A held-open panel
// must not survive a view the address has left, so the address gets its
// own listener: hashchange fires on every hash move, which a menu click
// answers for itself and a Back answers through popstate.
