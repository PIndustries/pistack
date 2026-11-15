tenantBox.addEventListener("change", () => {
  const inTenant = myProjects.filter(p => tenantOf(p) === tenantBox.value);
  if (!inTenant.length) return;
  const last = lastProject();
  switchProject((inTenant.find(p => p.name === last) || inTenant[0]).name);
});

