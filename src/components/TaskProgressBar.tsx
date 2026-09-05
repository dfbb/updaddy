import type { Ecosystem, PackageTask } from "../types";
import { cancelTask } from "../api/tauri";
import { useI18n } from "../i18n/useI18n";

export interface TaskProgressBarProps {
  ecosystem: Ecosystem;
  tasks: PackageTask[];
  onCancel?: (taskId: string) => void | Promise<void>;
}

function operationLabel(operation: string, t: (key: string, vars?: Record<string, string | number>) => string): string {
  return t(`operations.${operation}`, { operation });
}

function formatTransferBytes(bytes: number): string {
  if (bytes < 1_000_000) return `${(bytes / 1_000).toFixed(1)} KB`;
  if (bytes < 1_000_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
  return `${(bytes / 1_000_000_000).toFixed(2)} GB`;
}

function formatEta(seconds: number): string {
  const rounded = Math.max(0, Math.round(seconds));
  const hours = Math.floor(rounded / 3600);
  const minutes = Math.floor((rounded % 3600) / 60);
  const remainder = rounded % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(remainder).padStart(2, "0")}`
    : `${minutes}:${String(remainder).padStart(2, "0")}`;
}

/** 当前生态的活动任务；取消按钮不受全局操作禁用状态影响。 */
export default function TaskProgressBar({ ecosystem, tasks, onCancel }: TaskProgressBarProps) {
  const { t } = useI18n();
  const activeTasks = tasks.filter(
    (task) => task.ecosystem === ecosystem && (task.status === "pending" || task.status === "running"),
  );
  if (activeTasks.length === 0) return null;

  const current = activeTasks.find((task) => task.status === "running") ?? activeTasks[0];
  const completed = current.completed ?? 0;
  const total = current.total ?? 0;
  const determinate = total > 1;
  const value = determinate ? Math.max(0, Math.min(total, completed)) : undefined;
  const percent = determinate ? Math.min(100, Math.round((completed / total) * 100)) : undefined;
  const cancel = onCancel ?? ((taskId: string) => cancelTask(taskId));
  const downloadMessage = determinate && current.operation === "update"
    ? t(current.eta_seconds === undefined ? "task.download_detail_no_eta" : "task.download_detail", {
      downloaded: formatTransferBytes(completed),
      total: formatTransferBytes(total),
      percent: percent ?? 0,
      eta: formatEta(current.eta_seconds ?? 0),
    })
    : undefined;
  const message = current.message ?? downloadMessage ?? t(`task.${current.operation}_detail`, { name: current.name });

  return (
    <section className="task-progress" aria-label={t("task.progress_label")}>
      <div className="task-progress-main">
        <div className="task-progress-heading">
          <strong>{operationLabel(current.operation, t)}</strong>
          <span className="task-progress-package">{current.name === "*" ? t("task.ecosystem_scan") : current.name}</span>
          {determinate && <span className="task-progress-count">{percent}%</span>}
        </div>
        {determinate ? (
          <progress max={total} value={value} aria-label={t("task.progress_label")} />
        ) : (
          <div className="progress-indeterminate" aria-label={t("task.progress_label")} />
        )}
        <p className="task-progress-message">{message}</p>
      </div>
      <button
        type="button"
        className="button button-secondary task-cancel"
        onClick={() => void cancel(current.task_id)}
      >
        {t("task.cancel")}
      </button>
    </section>
  );
}

export { TaskProgressBar };
