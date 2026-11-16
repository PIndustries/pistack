    volumes() {
      const wrap = el("div", null, "panel");
      wrap.append(simpleTable([
        ["Name", v => v.name],
        ["GiB", v => v.spec && v.spec.size_gib],
        ["Device", v => v.status && v.status.device],
        ["Phase", v => v.status && v.status.phase],
      ], vols));
      pane.append(wrap);
      pane.append(el("p", "Attach a volume from the Volumes page. It shows up in the guest as a virtio disk.", "sub"));
    },
