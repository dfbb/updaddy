import { useEffect, useMemo, useState } from "react";
import { ListRestart, RotateCcw } from "lucide-react";
import { getStateSnapshot, listTaskAttempts, retryTask } from "../api/tauri";
import { useI18n } from "../i18n/useI18n";
import { applySnapshot, useAppStore } from "../state/appStore";
import type { Ecosystem, OperationBatch, PackageTask, TaskAttempt, TaskStatus } from "../types";
import LogViewer from "./LogViewer";

const ecosystems: Array<Ecosystem | "all"> = ["all", "homebrew", "npm", "pip", "gem", "rustup"];
const statuses: Array<TaskStatus | "all"> = ["all", "succeeded", "failed", "cancelled", "interrupted", "running", "pending"];
const operations = ["all", "scan", "update", "uninstall"] as const;
const periods = ["all", "day", "week", "month"] as const;

function taskMatches(task: PackageTask, status: TaskStatus | "all", operation: string): boolean {
  return (status === "all" || task.status === status)
    && (operation === "all" || task.operation === operation);
}

function batchCounts(batch: OperationBatch) {
  const tasks = batch.tasks.filter((task) => task.operation !== "measure_disk");
  return {
    total: tasks.length,
    succeeded: tasks.filter((task) => task.status === "succeeded").length,
    failed: tasks.filter((task) => task.status === "failed" || task.status === "interrupted").length,
    cancelled: tasks.filter((task) => task.status === "cancelled").length,
  };
}

function formatTime(value?: number): string {
  return value ? new Date(value * 1000).toLocaleString() : "-";
}

export default function HistoryPage() {
  const { t } = useI18n();
  const batches = useAppStore((state) => state.batches);
  const logs = useAppStore((state) => state.logs);
  const operationsDisabled = useAppStore((state) => state.operationsDisabled);
  const [ecosystem, setEcosystem] = useState<Ecosystem | "all">("all");
  const [status, setStatus] = useState<TaskStatus | "all">("all");
  const [operation, setOperation] = useState<(typeof operations)[number]>("all");
  const [period, setPeriod] = useState<(typeof periods)[number]>("all");
  const [retrying, setRetrying] = useState<string>();
  const [expanded, setExpanded] = useState<string>();
  const [attempts, setAttempts] = useState<Record<string, TaskAttempt[]>>({});
  const [error, setError] = useState<string>();

  useEffect(() => {
    void getStateSnapshot().then(applySnapshot).catch(() => undefined);
  }, []);

  const filtered = useMemo(() => {
    const now = Date.now() / 1000;
    const seconds = period === "day" ? 86400 : period === "week" ? 604800 : period === "month" ? 2592000 : undefined;
    return batches.filter((batch) => {
      if (ecosystem !== "all" && batch.ecosystem !== ecosystem) return false;
      if (seconds && batch.created_at < now - seconds) return false;
      return batch.tasks.some((task) => task.operation !== "measure_disk" && taskMatches(task, status, operation));
    });
  }, [batches, ecosystem, operation, period, status]);

  const retry = async (taskId: string) => {
    setRetrying(taskId);
    setError(undefined);
    try {
      await retryTask(taskId);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setRetrying(undefined);
    }
  };

  const toggleAttempts = async (taskId: string) => {
    if (expanded === taskId) {
      setExpanded(undefined);
      return;
    }
    setExpanded(taskId);
    if (!attempts[taskId]) {
      const next = await listTaskAttempts(taskId).catch(() => []);
      setAttempts((current) => ({ ...current, [taskId]: next }));
    }
  };

  return (
    <section className="history-page">
      <div className="page-heading"><div><p className="eyebrow">{t("navigation.history")}</p><h2>{t("history.title")}</h2></div></div>
      <div className="history-filters">
        <label className="field-inline"><span className="field-label">{t("history.filter_ecosystem")}</span><select value={ecosystem} onChange={(event) => setEcosystem(event.target.value as Ecosystem | "all")}>{ecosystems.map((item) => <option value={item} key={item}>{item === "all" ? t("history.all") : t(`ecosystems.${item}`)}</option>)}</select></label>
        <label className="field-inline"><span className="field-label">{t("history.filter_status")}</span><select value={status} onChange={(event) => setStatus(event.target.value as TaskStatus | "all")}>{statuses.map((item) => <option value={item} key={item}>{item === "all" ? t("history.all") : t(`task.${item}`)}</option>)}</select></label>
        <label className="field-inline"><span className="field-label">{t("history.filter_operation")}</span><select value={operation} onChange={(event) => setOperation(event.target.value as (typeof operations)[number])}>{operations.map((item) => <option value={item} key={item}>{item === "all" ? t("history.all") : t(`operations.${item}`)}</option>)}</select></label>
        <label className="field-inline"><span className="field-label">{t("history.filter_period")}</span><select value={period} onChange={(event) => setPeriod(event.target.value as (typeof periods)[number])}>{periods.map((item) => <option value={item} key={item}>{t(`history.period_${item}`)}</option>)}</select></label>
      </div>
      {error && <p className="settings-feedback error" role="alert">{error}</p>}
      <section className="history-section">
        <h3>{t("history.batches")}</h3>
        {filtered.length === 0 ? <p className="log-empty">{t("history.no_tasks")}</p> : (
          <div className="history-batch-list">
            {filtered.map((batch) => {
              const counts = batchCounts(batch);
              return (
                <article className="history-batch" key={batch.batch_id}>
                  <header className="history-batch-heading">
                    <div><strong>{t(`ecosystems.${batch.ecosystem}`)}</strong><time>{formatTime(batch.created_at)}</time></div>
                    <span>{t("history.batch_summary", counts)}</span>
                  </header>
                  <div className="history-task-list">
                    {batch.tasks.filter((task) => task.operation !== "measure_disk" && taskMatches(task, status, operation)).map((task) => (
                      <div className="history-task-block" key={task.task_id}>
                        <div className="history-task">
                          <div><strong>{task.name}</strong><span>{t(`operations.${task.operation}`)}{task.error ? ` · ${task.error}` : ""}</span></div>
                          <div className="history-task-meta">
                            <span className={`status-badge status-${task.status}`}>{t(`task.${task.status}`)}</span>
                            <button type="button" className="button button-secondary button-small" onClick={() => void toggleAttempts(task.task_id)}><ListRestart size={14} aria-hidden="true" />{t("history.attempts")}</button>
                            {(task.status === "failed" || task.status === "interrupted") && <button type="button" className="button button-secondary button-small" disabled={operationsDisabled || retrying === task.task_id} onClick={() => void retry(task.task_id)}><RotateCcw size={14} aria-hidden="true" />{t("history.retry")}</button>}
                          </div>
                        </div>
                        {expanded === task.task_id && <div className="attempt-list">{(attempts[task.task_id] ?? []).length === 0 ? <span>{t("history.no_attempts")}</span> : (attempts[task.task_id] ?? []).map((attempt) => <div key={attempt.id}><strong>#{attempt.attempt}</strong><span>{attempt.status}</span><span>{formatTime(attempt.started_at)} - {formatTime(attempt.finished_at)}</span></div>)}</div>}
                      </div>
                    ))}
                  </div>
                </article>
              );
            })}
          </div>
        )}
      </section>
      <section className="history-section"><h3>{t("logs.title")}</h3><LogViewer entries={logs} emptyLabel={t("logs.empty")} /></section>
    </section>
  );
}

export { batchCounts, taskMatches };
