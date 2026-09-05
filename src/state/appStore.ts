import { useSyncExternalStore } from "react";
import type { BackendEvent, Ecosystem, LogEntry, OperationBatch, PackageRecord, PackageTask, Settings, StateSnapshot, ThemeMode } from "../types";
import { resolveLocale, setTheme } from "../theme/theme";
type Listener = () => void;
export const defaultSettings: Settings = {
  visible_ecosystems: ["homebrew", "npm", "pip", "gem", "rustup"],
  theme: "system",
  locale: "en",
  schedule: undefined,
  login_item: false,
  proxy: { enabled: false, address: "", username: "", password: "" },
  logs: { level: "info", retain_command_output: false },
};
export interface AppState { locale: string; theme: ThemeMode; visibleEcosystems: Ecosystem[]; settings: Settings; packages: PackageRecord[]; tasks: PackageTask[]; batches: OperationBatch[]; logs: LogEntry[]; workers: Record<string, string>; activeTaskIds: string[]; operationsDisabled: boolean; totalUpdates: number; lastBatchSummary?: BackendEvent; }
let state: AppState = { locale: resolveLocale(), theme: "system", visibleEcosystems: defaultSettings.visible_ecosystems, settings: defaultSettings, packages: [], tasks: [], batches: [], logs: [], workers: {}, activeTaskIds: [], operationsDisabled: false, totalUpdates: 0 };
const listeners = new Set<Listener>();
const emit = () => listeners.forEach((l) => l());
const setState = (next: Partial<AppState>) => { state = { ...state, ...next }; emit(); };
export function useAppStore<T = AppState>(selector: (s: AppState) => T = ((s) => s as T)) { return useSyncExternalStore((l) => { listeners.add(l); return () => listeners.delete(l); }, () => selector(state), () => selector(state)); }
export function setSettings(settings: Settings) { setTheme(settings.theme); setState({ settings, locale: resolveLocale(settings.locale), theme: settings.theme, visibleEcosystems: settings.visible_ecosystems, totalUpdates: updateCount(state.packages, settings.visible_ecosystems) }); }
function isActive(status: PackageTask["status"] | string) { return status === "pending" || status === "running"; }
function derivedOperationsDisabled(tasks: PackageTask[], workers: Record<string, string>) {
  return tasks.some((task) => isActive(task.status)) || Object.values(workers).some((worker) => ["running", "scanning", "updating", "uninstalling"].includes(worker.toLowerCase()));
}
function updateCount(packages: PackageRecord[], visibleEcosystems: Ecosystem[]) {
  return packages.filter((p) => p.update_available && visibleEcosystems.includes(p.ecosystem)).length;
}
function effectiveVisible(configured: Ecosystem[], workers: Record<string, string>): Ecosystem[] {
  return configured.filter((ecosystem) => !["missing", "disabled"].includes((workers[ecosystem] ?? "").toLowerCase()));
}
export function applySnapshot(snapshot: StateSnapshot) {
  const active = snapshot.tasks.filter((t) => isActive(t.status)).map((t) => t.task_id);
  const visibleEcosystems = effectiveVisible(state.settings.visible_ecosystems, snapshot.workers);
  setState({ workers: snapshot.workers, visibleEcosystems, packages: snapshot.packages, logs: snapshot.logs, tasks: snapshot.tasks, batches: snapshot.batches ?? [], activeTaskIds: active, operationsDisabled: snapshot.active_tasks > 0 || derivedOperationsDisabled(snapshot.tasks, snapshot.workers), totalUpdates: updateCount(snapshot.packages, visibleEcosystems) });
}
export function applyEvent(name: string, payload: BackendEvent) {
  let tasks = state.tasks;
  if (name === "task-progress" && payload.task_id) {
    const existing = tasks.find((task) => task.task_id === payload.task_id);
    const next: PackageTask = {
      task_id: String(payload.task_id),
      ecosystem: payload.ecosystem as Ecosystem,
      name: existing?.name ?? String(payload.name ?? "*"),
      operation: existing?.operation ?? String(payload.operation ?? "scan"),
      status: payload.status as PackageTask["status"],
      error: payload.error,
      completed: typeof payload.completed === "number" ? payload.completed : existing?.completed,
      total: typeof payload.total === "number" ? payload.total : existing?.total,
      message: payload.message ?? existing?.message,
    };
    tasks = existing ? tasks.map((task) => task.task_id === payload.task_id ? { ...task, ...next } : task) : [...tasks, next];
  }
  const packages = name === "package-changed" && payload.package ? [...state.packages.filter((p) => p.id !== payload.package.id), payload.package] : name === "disk-usage" && payload.package_id ? state.packages.map((p) => p.id === payload.package_id ? { ...p, disk_usage: payload.disk_usage } : p) : state.packages;
  const logEntry = name === "log-entry" ? (payload.entry ?? (payload.message ? { message: payload.message, emitted_at: payload.emitted_at, stream: payload.stream ?? "system" } : undefined)) : undefined;
  const logs = logEntry ? [...state.logs, logEntry] : state.logs;
  const workers = name === "worker-state" ? { ...state.workers, [payload.ecosystem]: payload.state } : state.workers;
  const visibleEcosystems = name === "worker-state" ? effectiveVisible(state.settings.visible_ecosystems, workers) : state.visibleEcosystems;
  const active = tasks.filter((t) => isActive(t.status)).map((t) => t.task_id);
  setState({ tasks, packages, logs, workers, visibleEcosystems, activeTaskIds: active, operationsDisabled: derivedOperationsDisabled(tasks, workers), totalUpdates: updateCount(packages, visibleEcosystems), lastBatchSummary: name === "batch-summary" ? payload : state.lastBatchSummary });
}
export const getAppState = () => state;
