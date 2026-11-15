fetch("/").then((res) => res.json()).then((info) => {
  if (!info || !info.version) return;
  for (const el of document.querySelectorAll(".build")) {
    el.textContent = "build " + info.version;
    el.hidden = false;
  }
}).catch(() => { /* the page works without it */ });
