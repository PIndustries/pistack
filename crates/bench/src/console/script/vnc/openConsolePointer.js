  function pointer(ev, mask) {
    if (!fbW || stage === "dead") return;
    const box = canvas.getBoundingClientRect();
    if (!box.width || !box.height) return;
    const x = Math.max(0, Math.min(fbW - 1, Math.floor((ev.clientX - box.left) * (fbW / box.width))));
    const y = Math.max(0, Math.min(fbH - 1, Math.floor((ev.clientY - box.top) * (fbH / box.height))));
    const m = new Uint8Array(6);
    m[0] = 5;
    m[1] = mask & 255;
    m[2] = (x >> 8) & 255;
    m[3] = x & 255;
    m[4] = (y >> 8) & 255;
    m[5] = y & 255;
    send(m);
  }
  canvas.addEventListener("pointerdown", (ev) => {
    canvas.focus();
    ev.preventDefault();
    buttons |= (1 << ev.button);
    pointer(ev, buttons);
  });
  canvas.addEventListener("pointerup", (ev) => {
    buttons &= ~(1 << ev.button);
    pointer(ev, buttons);
  });
  canvas.addEventListener("pointermove", (ev) => { pointer(ev, buttons); });
  canvas.addEventListener("contextmenu", (ev) => ev.preventDefault());
