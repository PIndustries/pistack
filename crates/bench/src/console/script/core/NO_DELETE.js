// Pages whose rows are not deleted from here: a host leaves by being
// drained and removed, and these pages draw their own rows.
const NO_DELETE = new Set(["hosts", "dns", "balancers", "overview", "topology"]);

