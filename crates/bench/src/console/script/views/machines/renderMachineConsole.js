    console() {
      const card = el("div", null, "detail");
      card.style.width = "100%";
      pane.append(card);
      if (power === "on") openConsole(card, machine.id);
      else card.append(el("p", "The machine is off. Start it to open its console.", "sub"));
    },
