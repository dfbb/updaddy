export type Ecosystem = "homebrew" | "npm" | "pip" | "gem" | "rustup";
export type ThemeMode = "system" | "light" | "dark";
export type TaskStatus = "pending" | "running" | "succeeded" | "failed" | "cancelled";
export interface PackageRecord { id: string; ecosystem: Ecosystem; resource_kind: string; name: string; current_version?: string; target_version?: string; disk_usage?: { bytes: number; scanned_at: number; status: string }; update_available: boolean; }
export interface PackageTask { task_id: string; ecosystem: Ecosystem; name: string; operation: string; status: TaskStatus; error?: string; }
export interface LogEntry { message: string; emitted_at: number; stream: string; }
export interface StateSnapshot { workers: Record<string, string>; packages: PackageRecord[]; logs: LogEntry[]; active_tasks: number; tasks: PackageTask[]; }
export interface Settings { visible_ecosystems: Ecosystem[]; theme: ThemeMode; locale: string; schedule?: string; login_item: boolean; }
export interface BackendEvent { ecosystem: Ecosystem; sequence: number; emitted_at: number; [key: string]: any; }
