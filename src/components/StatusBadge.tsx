import type { ReactNode } from "react";
import type { PackageRecord, TaskStatus } from "../types";
import { useI18n } from "../i18n/useI18n";

export type BadgeStatus =
  | TaskStatus
  | "update_available"
  | "up_to_date"
  | "measuring"
  | "ready"
  | "unavailable"
  | "missing"
  | "idle"
  | "running"
  | "failed";

interface StatusBadgeProps {
  status: BadgeStatus | string;
  updateAvailable?: boolean;
  children?: ReactNode;
}

const statusKey: Record<string, string> = {
  pending: "task.pending",
  running: "task.running",
  succeeded: "task.succeeded",
  failed: "task.failed",
  cancelled: "task.cancelled",
  interrupted: "task.interrupted",
  update_available: "package.update_available",
  up_to_date: "package.up_to_date",
  measuring: "package.measuring",
  ready: "package.ready",
  unavailable: "package.unavailable",
  missing: "package.unavailable",
  idle: "worker.idle",
};

function normalizeStatus(status: string, updateAvailable?: boolean): string {
  if (updateAvailable) return "update_available";
  return status.toLowerCase().replaceAll("-", "_");
}

/** 用统一颜色和本地化文本显示任务、包和 worker 状态。 */
export function StatusBadge({ status, updateAvailable, children }: StatusBadgeProps) {
  const { t } = useI18n();
  const normalized = normalizeStatus(status, updateAvailable);
  const label = children ?? t(statusKey[normalized] ?? normalized);
  return (
    <span className={`status-badge status-${normalized}`} role="status">
      {label}
    </span>
  );
}

export function packageStatus(packageRecord: Pick<PackageRecord, "update_available">): BadgeStatus {
  return packageRecord.update_available ? "update_available" : "up_to_date";
}

export default StatusBadge;
