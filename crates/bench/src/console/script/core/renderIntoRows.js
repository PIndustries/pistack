  if (rowAction || view.rules) {
    const head = table.querySelector("tr");
    if (rowAction) head.append(el("th", ""));
    const rows = body.querySelectorAll("tr");
    if (!items.length) {
      const td = rows[0] && rows[0].firstChild;
      if (td) td.colSpan = view.cols.length + (rowAction ? 1 : 0);
    } else {
      items.forEach((item, idx) => {
        const tr = rows[idx];
        if (!tr) return;
        if (current === "machines") {
          const nameCell = tr.firstChild;
          if (nameCell) {
            nameCell.replaceChildren();
            const btn = el("button", item.name || "—");
            btn.type = "button";
            btn.className = "linkish";
            btn.addEventListener("click", () => { openMachine = item.id; render(); });
            nameCell.append(btn);
          }
        }
        if (view.rules) {
          const rules = (item.spec && item.spec.rules) || [];
          const cell = tr.children[2];
          if (cell) {
            cell.replaceChildren(document.createTextNode(String(rules.length)));
            if (rules.length) {
              const ul = el("ul"); ul.className = "rules";
              for (const r of rules) {
                const li = el("li", ruleLine(r));
                if (!readOnly && r.id) {
                  const x = el("button", "×", "x"); x.type = "button";
                  x.title = "Remove this rule";
                  x.setAttribute("aria-label", "Remove rule " + ruleLine(r));
                  x.addEventListener("click", async () => {
                    if (!confirm("Remove the rule " + ruleLine(r) + " from " + item.name + "?")) return;
                    setErr("");
                    try { await api("DELETE", view.path + "/" + item.id + "/rules/" + r.id); await render(); }
                    catch (e) { setErr(e.message); }
                  });
                  li.append(x);
                }
                ul.append(li);
              }
              cell.append(ul);
            }
          }
        }
        if (rowAction) {
          const td = el("td");
          if (view.actions) {
            const start = el("button", "Start"); start.type = "button";
            const stop = el("button", "Stop"); stop.type = "button";
            const del = el("button", "Delete"); del.type = "button"; del.className = "danger";
            start.addEventListener("click", async () => {
              setErr("");
              try { await api("POST", view.path + "/" + item.id + "/start"); await render(); }
              catch (e) { setErr(e.message); }
            });
            stop.addEventListener("click", async () => {
              setErr("");
              try { await api("POST", view.path + "/" + item.id + "/stop"); await render(); }
              catch (e) { setErr(e.message); }
            });
            del.addEventListener("click", async () => {
              if (!confirm("Delete " + item.name + "?")) return;
              setErr("");
              try { await api("DELETE", view.path + "/" + item.id); await render(); }
              catch (e) { setErr(e.message); }
            });
            td.append(start, document.createTextNode(" "), stop, document.createTextNode(" "), del);
          }
          if (view.apply) {
            const apply = el("button", "Apply"); apply.type = "button";
            apply.addEventListener("click", async () => {
              setErr("");
              apply.disabled = true;
              try {
                await api("POST", view.path + "/" + item.id + "/apply");
                await render();
              } catch (e) { setErr(e.message); }
              apply.disabled = false;
            });
            if (td.childNodes.length) td.append(document.createTextNode(" "));
            td.append(apply);
          }
          if (canDelete) {
            const del = el("button", "Delete"); del.type = "button"; del.className = "danger";
            del.addEventListener("click", async () => {
              if (!confirm("Delete " + (item.name || item.id) + "? This cannot be undone.")) return;
              setErr("");
              del.disabled = true;
              try { await api("DELETE", view.path + "/" + item.id); await render(); }
              catch (e) { setErr(e.message); del.disabled = false; }
            });
            if (td.childNodes.length) td.append(document.createTextNode(" "));
            td.append(del);
          }
          const history = (item.status && item.status.history) || [];
          if (view.rollback && history.length > 1) {
            const back = el("button", "Roll back"); back.type = "button";
            back.addEventListener("click", async () => {
              if (!confirm("Roll " + item.name + " back to its previous revision?")) return;
              setErr("");
              back.disabled = true;
              try {
                await api("POST", view.path + "/" + item.id + "/rollback");
                await render();
              } catch (e) { setErr(e.message); }
              back.disabled = false;
            });
            td.append(document.createTextNode(" "), back);
          }
          tr.append(td);
        }
      });
    }
  }
