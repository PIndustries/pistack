    console() {
      // A soft pass holds while a console is open (machineHeld / focus in the
      // page), so this pane is never rebuilt out from under a live websocket;
      // a fresh draw opens the console afresh.
      const card = el("div", null, "detail");
      card.style.width = "100%";
      pane.append(card);
      if (power === "on") openConsole(card, machine.id);
      else card.append(el("p", "The machine is off. Start it to open its console.", "sub"));
    },
