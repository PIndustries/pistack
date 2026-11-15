  api("GET", "/compute/machines/" + encodeURIComponent(id) + "/console").then((data) => {
    const path = (data && data.websocket) || "";
    if (!path.includes("console/vnc")) {
      status.textContent = "console/vnc path missing";
      return;
    }
    const proto = location.protocol === "https:" ? "wss://" : "ws://";
    ws = new WebSocket(proto + location.host + path);
    ws.binaryType = "arraybuffer";
    ws.addEventListener("message", (ev) => {
      if (!(ev.data instanceof ArrayBuffer)) { fail("console frame was not binary"); return; }
      append(new Uint8Array(ev.data));
      pump();
    });
    ws.addEventListener("close", () => {
      if (stage !== "dead") status.textContent = "Console closed";
    });
    ws.addEventListener("error", () => { status.textContent = "Console connection failed"; });
  }).catch((e) => { status.textContent = e.message; });
}

// The tab last opened on a machine page, kept while moving between machines.
