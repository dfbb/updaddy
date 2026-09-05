export type Ecosystem = "homebrew" | "npm" | "pip" | "gem" | "rustup";
export type ThemeMode = "system" | "light" | "dark";
export type ProxyMode = "direct" | "socks5";
export type LogLevel = "error" | "warn" | "info" | "debug";
export type TaskStatus = "pending" | "running" | "interrupted" | "succeeded" | "failed" | "cancelled";
export interface PackageRecord { id: string; ecosystem: Ecosystem; resource_kind: string; name: string; current_version?: string; target_version?: string; disk_usage?: { bytes: number; scanned_at: number; status: string }; update_available: boolean; }
export interface PackageTask { task_id: string; ecosystem: Ecosystem; name: string; operation: string; status: TaskStatus; error?: string; completed?: number; total?: number; eta_seconds?: number; message?: string; }
export interface OperationBatch { batch_id: string; ecosystem: Ecosystem; tasks: PackageTask[]; created_at: number; }
export interface LogEntry { message: string; emitted_at: number; stream: string; }
export interface StateSnapshot { workers: Record<string, string>; packages: PackageRecord[]; logs: LogEntry[]; active_tasks: number; tasks: PackageTask[]; batches?: OperationBatch[]; }
export interface ProxySettings { enabled: boolean; address: string; username?: string; password?: string; }
export interface LogSettings { level: LogLevel; retain_command_output: boolean; }
export interface Settings { visible_ecosystems: Ecosystem[]; theme: ThemeMode; locale: string; schedule?: string; login_item: boolean; proxy: ProxySettings; logs: LogSettings; }
export interface TaskAttempt { id: number; attempt: number; status: string; started_at?: number; finished_at?: number; }
export interface BackendEvent { ecosystem: Ecosystem; sequence: number; emitted_at: number; [key: string]: any; }
