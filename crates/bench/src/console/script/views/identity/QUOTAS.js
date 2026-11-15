// ---- limits and roles ------------------------------------------------------

// The order the limits are shown in, and how each is counted: compute, then
// storage, then network, then the rest. A resource not named here keeps its
// own name and counts one at a time.
const QUOTAS = [
  ["vcpus", "vCPUs", ""],
  ["memory_mib", "Memory", "mib"],
  ["machines", "Machines", ""],
  ["disk_gib", "Disk", "gib"],
  ["volumes", "Volumes", ""],
  ["volumes_gib", "Volume space", "gib"],
  ["snapshots", "Snapshots", ""],
  ["backups", "Backups", ""],
  ["backups_gib", "Backup space", "gib"],
  ["images", "Images", ""],
  ["images_bytes", "Image data", "bytes"],
  ["buckets", "Buckets", ""],
  ["secrets", "Secrets", ""],
  ["certificates", "Certificates", ""],
  ["access_keys", "Access keys", ""],
  ["networks", "Networks", ""],
  ["subnets", "Subnets", ""],
  ["routers", "Routers", ""],
  ["security_groups", "Security groups", ""],
  ["ports", "Ports", ""],
  ["floating_ips", "Floating IPs", ""],
];
