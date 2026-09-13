import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { BackendEvent, Ecosystem, LogEntry, PackageRecord, Settings, StateSnapshot } from "../types";
export const invokeTask = (command: string, payload?: Record<string, unknown>) => invoke<string | string[]>(command, payload);
export const getStateSnapshot = () => invoke<StateSnapshot>("get_state_snapshot");
export const getSettings = () => invoke<Settings>("get_settings");
export const saveSettings = (settings: Settings) => invoke<void>("save_settings", { settings });
export const cancelTask = (taskId: string) => invoke<void>("cancel_task", { taskId });
export const scanEcosystem = (ecosystem: Ecosystem) => invoke<string>("scan_ecosystem", { ecosystem });
export const scanAllVisible = () => invoke<string[]>("scan_all_visible");
export const updatePackage = (packageRecord: Pick<PackageRecord, "ecosystem" | "name">) => invoke<string>("update_package", { package: packageRecord });
export const uninstallPackage = (packageRecord: Pick<PackageRecord, "ecosystem" | "name">) => invoke<string>("uninstall_package", { package: packageRecord });
export const updateAllVisible = () => invoke<string[]>("update_all_visible");
export const updateEcosystem = (ecosystem: Ecosystem) => invoke<string[]>("update_ecosystem", { ecosystem });
export const refreshDiskUsage = (packageRecord: PackageRecord) => invoke<string>("refresh_disk_usage", { package: packageRecord });
export const setLoginItem = (enabled: boolean) => invoke<void>("set_login_item", { enabled });
export const cleanupExpiredLogs = () => invoke<number>("cleanup_expired_logs");
export const retryTask = (taskId: string) => invoke<string>("retry_task", { taskId });
export const listTaskLogs = (taskId: string) => invoke<LogEntry[]>("list_task_logs", { taskId });
export async function subscribeToBackendEvents(onEvent: (name: string, payload: BackendEvent) => void): Promise<UnlistenFn[]> {
  const names = ["worker-state", "task-progress", "package-changed", "package-removed", "disk-usage", "log-entry", "batch-summary"];
  const last = new Map<string, number>();
  return Promise.all(names.map((name) => listen<BackendEvent>(name, ({ payload }) => {
    // Log entries do not carry an ecosystem sequence; retain every entry in arrival order.
    if (name === "log-entry" && payload.sequence === undefined) {
      onEvent(name, payload);
      return;
    }
    const key = payload.ecosystem ?? "global";
    const sequence = Number(payload.sequence ?? -1);
    const prev = last.get(key) ?? -1;
    if (sequence <= prev) return;
    last.set(key, sequence);
    onEvent(name, payload);
  })));
}
