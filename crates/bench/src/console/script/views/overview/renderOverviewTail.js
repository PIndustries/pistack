
  if (auditPage !== null) {
    root.append(el("h3", "Recent activity"));
    const ap = el("div", null, "panel");
    ap.append(activityFeed(itemsOf(auditPage)));
    root.append(ap);
  }
  if (failed.length) setErr("Could not read: " + failed.join(", ") + ".");
}

// `soft` is a background reload: the page is built off-screen and swapped
// in when it is complete, so nothing flickers, and a failed reload leaves
// what was on screen.
