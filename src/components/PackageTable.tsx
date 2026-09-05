import { useMemo, useState } from "react";
import { ArrowDown, ArrowUp, CircleArrowUp, Trash2 } from "lucide-react";
import { uninstallPackage, updatePackage } from "../api/tauri";
import type { Ecosystem, PackageRecord, PackageTask } from "../types";
import { useI18n } from "../i18n/useI18n";
import ConfirmUninstallDialog from "./ConfirmUninstallDialog";
import EmptyState from "./EmptyState";
import StatusBadge, { packageStatus } from "./StatusBadge";

type SortMode = "default" | "disk";
type DiskDirection = "asc" | "desc";

export interface PackageInput extends Partial<PackageRecord> {
  id: string;
  name: string;
  ecosystem?: Ecosystem;
  resourceKind?: string;
  currentVersion?: string;
  targetVersion?: string;
  diskUsage?: { bytes: number; scanned_at?: number; status?: string };
  diskBytes?: number;
  updateAvailable?: boolean;
}

export interface PackageTableProps {
  packages: PackageInput[];
  ecosystem?: Ecosystem;
  disabled?: boolean;
  activeTaskIds?: string[];
  activeTasks?: PackageTask[];
  onUpdate?: (packageRecord: PackageRecord) => void | Promise<void>;
  onUninstall?: (packageRecord: PackageRecord) => void | Promise<void>;
}

export function normalizePackageRecord(input: PackageInput): PackageRecord {
  const prefix = input.ecosystem ?? input.id.split(":", 1)[0];
  const ecosystem: Ecosystem = ["homebrew", "npm", "pip", "gem", "rustup"].includes(prefix as Ecosystem) ? prefix as Ecosystem : "npm";
  const disk = input.disk_usage ?? input.diskUsage ?? (input.diskBytes === undefined ? undefined : { bytes: input.diskBytes, scanned_at: 0, status: "ready" });
  return {
    id: input.id,
    ecosystem,
    resource_kind: input.resource_kind ?? input.resourceKind ?? "package",
    name: input.name,
    current_version: input.current_version ?? input.currentVersion,
    target_version: input.target_version ?? input.targetVersion,
    disk_usage: disk ? { bytes: disk.bytes, scanned_at: disk.scanned_at ?? 0, status: disk.status ?? "ready" } : undefined,
    update_available: input.update_available ?? input.updateAvailable ?? false,
  };
}

function diskBytes(packageRecord: PackageRecord): number | undefined {
  return packageRecord.disk_usage?.status === "ready" ? packageRecord.disk_usage.bytes : undefined;
}

function formatBytes(bytes: number | undefined, t: (key: string, vars?: Record<string, string | number>) => string): string {
  if (bytes === undefined) return t("package.disk_unavailable");
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = -1;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 10 || unit === 0 ? 0 : 1)} ${units[unit]}`;
}

function displayVersion(value: string | undefined, unknown: string): string {
  return value || unknown;
}

function packageBusy(packageRecord: PackageRecord, activeTaskIds: Set<string>, activeTasks: PackageTask[]): boolean {
  return activeTaskIds.has(packageRecord.id) || activeTasks.some((task) => task.ecosystem === packageRecord.ecosystem && task.name === packageRecord.name && (task.status === "pending" || task.status === "running"));
}

export default function PackageTable({
  packages,
  ecosystem,
  disabled = false,
  activeTaskIds = [],
  activeTasks = [],
  onUpdate,
  onUninstall,
}: PackageTableProps) {
  const { t } = useI18n();
  const [sortMode, setSortMode] = useState<SortMode>("default");
  const [diskDirection, setDiskDirection] = useState<DiskDirection>("desc");
  const [pendingUninstall, setPendingUninstall] = useState<PackageRecord | null>(null);
  const active = useMemo(() => new Set(activeTaskIds), [activeTaskIds]);
  const normalizedPackages = useMemo(() => packages.map(normalizePackageRecord), [packages]);

  const sortedPackages = useMemo(() => {
    const filtered = ecosystem ? normalizedPackages.filter((item) => item.ecosystem === ecosystem) : normalizedPackages;
    return [...filtered].sort((a, b) => {
      if (sortMode === "disk") {
        const left = diskBytes(a) ?? -1;
        const right = diskBytes(b) ?? -1;
        if (left !== right) return diskDirection === "asc" ? left - right : right - left;
      } else if (a.update_available !== b.update_available) {
        return a.update_available ? -1 : 1;
      }
      return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
    });
  }, [diskDirection, ecosystem, normalizedPackages, sortMode]);

  const handleDiskSort = () => {
    if (sortMode === "disk") {
      setDiskDirection((direction) => (direction === "desc" ? "asc" : "desc"));
    } else {
      setSortMode("disk");
      setDiskDirection("desc");
    }
  };

  const resetSort = () => setSortMode("default");
  const update = onUpdate ?? ((item: PackageRecord) => updatePackage({ ecosystem: item.ecosystem, name: item.name }));
  const uninstall = onUninstall ?? ((item: PackageRecord) => uninstallPackage({ ecosystem: item.ecosystem, name: item.name }));

  if (sortedPackages.length === 0) return <EmptyState />;

  return (
    <>
      <div className="package-table-wrap">
        <table className="package-table">
          <caption className="sr-only">{t("package.table_caption")}</caption>
          <thead>
            <tr>
              <th scope="col">
                <button type="button" className="table-sort-button" onClick={resetSort}>
                  {t("package.name")}
                </button>
              </th>
              <th scope="col">{t("package.current_version")}</th>
              <th scope="col">{t("package.target_version")}</th>
              <th scope="col" aria-sort={sortMode === "disk" ? (diskDirection === "asc" ? "ascending" : "descending") : "none"}>
                <button type="button" className="table-sort-button" onClick={handleDiskSort}>
                  {t("package.disk_usage")}
                  {sortMode === "disk" && (diskDirection === "asc" ? <ArrowUp size={13} aria-hidden="true" /> : <ArrowDown size={13} aria-hidden="true" />)}
                </button>
              </th>
              <th scope="col">{t("package.status")}</th>
              <th scope="col">{t("package.actions")}</th>
            </tr>
          </thead>
          <tbody>
            {sortedPackages.map((item) => {
              const busy = disabled || packageBusy(item, active, activeTasks);
              const disk = item.disk_usage;
              return (
                <tr key={item.id} data-package-id={item.id}>
                  <th scope="row">
                    <div className="package-name">{item.name}</div>
                    <small className="package-kind">{t(`resource.${item.resource_kind}`)}</small>
                  </th>
                  <td>{displayVersion(item.current_version, t("package.unknown_version"))}</td>
                  <td>{item.update_available ? displayVersion(item.target_version, t("package.pending_target")) : "—"}</td>
                  <td>
                    <span className={disk?.status === "measuring" ? "disk-measuring" : undefined}>
                      {disk?.status === "measuring" ? t("package.disk_measuring") : formatBytes(diskBytes(item), t)}
                    </span>
                  </td>
                  <td><StatusBadge status={packageStatus(item)} /></td>
                  <td>
                    <div className="package-actions">
                      {item.update_available && (
                        <button type="button" className="button button-primary button-small" disabled={busy} onClick={() => void update(item)}>
                          <CircleArrowUp size={14} aria-hidden="true" />{t("buttons.update")}
                        </button>
                      )}
                      <button type="button" className="button button-danger button-small" disabled={busy} onClick={() => setPendingUninstall(item)}>
                        <Trash2 size={14} aria-hidden="true" />{t("buttons.uninstall")}
                      </button>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <ConfirmUninstallDialog
        packageRecord={pendingUninstall}
        disabled={disabled || (pendingUninstall ? packageBusy(pendingUninstall, active, activeTasks) : false)}
        onCancel={() => setPendingUninstall(null)}
        onConfirm={async (item) => {
          await uninstall(item);
          setPendingUninstall(null);
        }}
      />
    </>
  );
}

export { formatBytes, PackageTable };
