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
export interface AppState { locale: string; theme: ThemeMode; visibleEcosystems: Ecosystem[]; settings: Settings; packages: PackageRecord[]; tasks: PackageTask[]; batches: OperationBatch[]; logs: LogEntry[]; workers: Record<string, string>; activeTaskIds: string[]; operationsDisabled: boolean; totalUpdates: number; historyRevision: number; lastBatchSummary?: BackendEvent; }
let state: AppState = { locale: resolveLocale(), theme: "system", visibleEcosystems: defaultSettings.visible_ecosystems, settings: defaultSettings, packages: [], tasks: [], batches: [], logs: [], workers: {}, activeTaskIds: [], operationsDisabled: false, totalUpdates: 0, historyRevision: 0 };
const listeners = new Set<Listener>();
const emit = () => listeners.forEach((l) => l());
const setState = (next: Partial<AppState>) => { state = { ...state, ...next }; emit(); };
export function useAppStore<T = AppState>(selector: (s: AppState) => T = ((s) => s as T)) { return useSyncExternalStore((l) => { listeners.add(l); return () => listeners.delete(l); }, () => selector(state), () => selector(state)); }
export function setSettings(settings: Settings) { setTheme(settings.theme); setState({ settings, locale: resolveLocale(settings.locale), theme: settings.theme, visibleEcosystems: settings.visible_ecosystems, totalUpdates: updateCount(state.packages, settings.visible_ecosystems) }); }
function isActive(status: PackageTask["status"] | string) { return status === "pending" || status === "running"; }
function isVisibleTask(task: PackageTask) { return isActive(task.status) && task.operation !== "measure_disk"; }
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
  const tasks = snapshot.tasks.filter(isVisibleTask);
  const active = tasks.map((t) => t.task_id);
  const visibleEcosystems = effectiveVisible(state.settings.visible_ecosystems, snapshot.workers);
  setState({ workers: snapshot.workers, visibleEcosystems, packages: snapshot.packages, logs: snapshot.logs, tasks, batches: snapshot.batches ?? [], activeTaskIds: active, operationsDisabled: snapshot.active_tasks > 0 || derivedOperationsDisabled(tasks, snapshot.workers), totalUpdates: updateCount(snapshot.packages, visibleEcosystems) });
}
function reduceEvent(current: AppState, name: string, payload: BackendEvent): AppState {
  let tasks = current.tasks;
  let batches = current.batches;
  let historyRevision = current.historyRevision;
  if (name === "task-progress" && payload.task_id) {
    const existing = tasks.find((task) => task.task_id === payload.task_id);
    const historyTask = batches.flatMap((batch) => batch.tasks).find((task) => task.task_id === payload.task_id);
    const next: PackageTask = {
      task_id: String(payload.task_id),
      ecosystem: payload.ecosystem as Ecosystem,
      name: existing?.name ?? String(payload.name ?? "*"),
      operation: existing?.operation ?? String(payload.operation ?? "scan"),
      status: payload.status as PackageTask["status"],
      error: payload.error,
      completed: typeof payload.completed === "number" ? payload.completed : existing?.completed,
      total: typeof payload.total === "number" ? payload.total : existing?.total,
      eta_seconds: "eta_seconds" in payload ? (typeof payload.eta_seconds === "number" ? payload.eta_seconds : undefined) : existing?.eta_seconds,
      phase: "phase" in payload ? (typeof payload.phase === "string" ? payload.phase : undefined) : existing?.phase,
      message: payload.message ?? existing?.message,
    };
    if (existing?.status !== next.status && historyTask?.status !== next.status) {
      historyRevision += 1;
    }
    batches = batches.map((batch) => ({
      ...batch,
      tasks: batch.tasks.map((task) => task.task_id === payload.task_id ? {
        ...task,
        status: next.status,
        error: next.error,
        completed: next.completed,
        total: next.total,
        eta_seconds: next.eta_seconds,
        phase: next.phase,
        message: next.message,
      } : task),
    }));
    if (isVisibleTask(next)) {
      tasks = existing ? tasks.map((task) => task.task_id === payload.task_id ? { ...task, ...next } : task) : [...tasks, next];
    } else if (existing) {
      tasks = tasks.filter((task) => task.task_id !== payload.task_id);
    }
  }
  const packages = name === "package-changed" && payload.package
    ? [...current.packages.filter((p) => p.id !== payload.package.id), payload.package]
    : name === "package-removed" && payload.package_id
      ? current.packages.filter((p) => p.id !== payload.package_id)
      : name === "disk-usage" && payload.package_id
        ? current.packages.map((p) => p.id === payload.package_id ? { ...p, disk_usage: payload.disk_usage } : p)
        : current.packages;
  const logEntry = name === "log-entry" ? (payload.entry
    ? { ...payload.entry, task_id: payload.task_id ? String(payload.task_id) : payload.entry.task_id }
    : (payload.message ? { message: payload.message, emitted_at: payload.emitted_at, stream: payload.stream ?? "system", task_id: payload.task_id ? String(payload.task_id) : undefined } : undefined)) : undefined;
  const logs = logEntry ? [...current.logs, logEntry] : current.logs;
  const workers = name === "worker-state" ? { ...current.workers, [payload.ecosystem]: payload.state } : current.workers;
  const visibleEcosystems = name === "worker-state" ? effectiveVisible(current.settings.visible_ecosystems, workers) : current.visibleEcosystems;
  return { ...current, tasks, batches, packages, logs, workers, visibleEcosystems, historyRevision, lastBatchSummary: name === "batch-summary" ? payload : current.lastBatchSummary };
}
export function applyEvents(events: ReadonlyArray<readonly [string, BackendEvent]>) {
  if (events.length === 0) return;
  const next = events.reduce((current, [name, payload]) => reduceEvent(current, name, payload), state);
  const active = next.tasks.map((task) => task.task_id);
  state = { ...next, activeTaskIds: active, operationsDisabled: derivedOperationsDisabled(next.tasks, next.workers), totalUpdates: updateCount(next.packages, next.visibleEcosystems) };
  emit();
}
export function applyEvent(name: string, payload: BackendEvent) { applyEvents([[name, payload]]); }
export const getAppState = () => state;
