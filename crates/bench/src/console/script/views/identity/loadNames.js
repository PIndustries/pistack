function loadNames(view, gen) {
  const roles = view === "users" ? api("GET", "/identity/roles?limit=1000").catch(() => null) : Promise.resolve(null);
  return Promise.all([roles, api("GET", "/identity/projects?limit=1000").catch(() => null)]).then((pages) => {
    if (gen !== renderGen) return [[], []];
    for (const p of itemsOf(pages[1])) projectNames.set(String(p.id), p.name);
    return [itemsOf(pages[0]), itemsOf(pages[1])].map((l) => l || []);
  });
}
// Where a binding applies: the project it names, or the whole cloud.
