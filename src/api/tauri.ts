import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { BackendEvent, Settings, StateSnapshot } from "../types";
export const invokeTask = (command: string, payload?: Record<string, unknown>) => invoke<string | string[]>(command, payload);
export const getStateSnapshot = () => invoke<StateSnapshot>("get_state_snapshot");
export const getSettings = () => invoke<Settings>("get_settings");
export const saveSettings = (settings: Settings) => invoke<void>("save_settings", { settings });
export const cancelTask = (taskId: string) => invoke<void>("cancel_task", { taskId });
export async function subscribeToBackendEvents(onEvent: (name: string, payload: BackendEvent) => void): Promise<UnlistenFn[]> { const names = ["worker-state", "task-progress", "package-changed", "disk-usage", "log-entry", "batch-summary"]; const last = new Map<string, number>(); return Promise.all(names.map((name) => listen<BackendEvent>(name, ({ payload }) => { const prev = last.get(payload.ecosystem) ?? -1; if (payload.sequence <= prev) return; last.set(payload.ecosystem, payload.sequence); onEvent(name, payload); }))); }
