# Lab nodes

Real hardware for Phase 1 (KVM, libvirt, OVN, LVM). Nodes never accept inbound
connections for this; they run a GitHub Actions self-hosted runner that pulls
jobs from this private repo.

A lab node is a machine you control: an x86 KVM host, or an ARM64 host. The
runner on that machine pulls jobs. The repo does not list private host names.

## One-time setup per node

```sh
git clone https://github.com/PIndustries/pistack && cd pistack
sudo ./scripts/lab-bootstrap.sh --report    # what this box has
sudo ./scripts/lab-bootstrap.sh --install   # KVM, libvirt, OVS/OVN, LVM, Rust
TOKEN=$(gh api -X POST repos/PIndustries/pistack/actions/runners/registration-token -q .token)
sudo ./scripts/lab-bootstrap.sh --runner "$TOKEN"
```

Every push to `main` or `lab/*` then runs `.github/workflows/lab.yml` on each
node: hardware report, release build, full test suite, and a server+agent
smoke test driven by `pi`. Logs are uploaded as workflow artifacts.

Security: the runner runs as the unprivileged `pistack` user (in the `kvm` and
`libvirt` groups). Only use self-hosted runners with a private repository.
