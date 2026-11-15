function onCreate(form, go, work) {
  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    setErr("");
    go.disabled = true;
    try {
      // A work that redrew its own panel and says so needs no page build.
      if (await work() !== true) await render();
    } catch (e) {
      setErr(e.message);
    }
    go.disabled = false;
  });
}

