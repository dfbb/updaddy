import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { OperationBatch, PackageTask } from "../types";
import { applyEvent, applySnapshot } from "../state/appStore";
import HistoryPage, { batchCounts, taskMatches } from "./HistoryPage";

const api = vi.hoisted(() => ({
  getStateSnapshot: vi.fn(),
  listTaskLogs: vi.fn(),
  retryTask: vi.fn(),
}));

vi.mock("../api/tauri", () => api);

const update: PackageTask = { task_id: "1", ecosystem: "npm", name: "eslint", operation: "update", status: "succeeded" };
const failed: PackageTask = { task_id: "2", ecosystem: "npm", name: "typescript", operation: "uninstall", status: "failed" };
const measurement: PackageTask = { task_id: "3", ecosystem: "npm", name: "eslint", operation: "measure_disk", status: "succeeded" };

describe("HistoryPage helpers", () => {
  it("summarizes user operations without counting disk child tasks", () => {
    const batch: OperationBatch = { batch_id: "batch", ecosystem: "npm", created_at: 1, tasks: [update, failed, measurement] };
    expect(batchCounts(batch)).toEqual({ total: 2, succeeded: 1, failed: 1, cancelled: 0 });
  });

  it("filters tasks by status and operation", () => {
    expect(taskMatches(failed, "failed", "uninstall")).toBe(true);
    expect(taskMatches(failed, "succeeded", "uninstall")).toBe(false);
  });

  it("opens task logs and displays the retained command error", async () => {
    const batch: OperationBatch = { batch_id: "batch", ecosystem: "npm", created_at: 1, tasks: [failed] };
    api.getStateSnapshot.mockResolvedValue({ workers: {}, packages: [], logs: [], active_tasks: 0, tasks: [], batches: [batch] });
    api.listTaskLogs.mockResolvedValue([{ message: "permission denied: /usr/local", emitted_at: 10, stream: "stderr" }]);
    render(<HistoryPage />);

    await userEvent.click(await screen.findByRole("button", { name: /logs|日志/i }));

    expect(api.listTaskLogs).toHaveBeenCalledWith("2");
    expect(await screen.findByText("permission denied: /usr/local")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /attempts|尝试记录/i })).not.toBeInTheDocument();
  });

  it("updates a visible task when its backend status changes", async () => {
    const running: PackageTask = { task_id: "live", ecosystem: "gem", name: "rake@13.0.6", operation: "update", status: "running" };
    const runningBatch: OperationBatch = { batch_id: "live-batch", ecosystem: "gem", created_at: 1, tasks: [running] };
    const failedBatch: OperationBatch = { ...runningBatch, tasks: [{ ...running, status: "failed", error: "command_failed" }] };
    const snapshot = (batch: OperationBatch) => ({ workers: {}, packages: [], logs: [], active_tasks: batch.tasks[0].status === "running" ? 1 : 0, tasks: batch.tasks, batches: [batch] });
    applySnapshot(snapshot(runningBatch));
    api.getStateSnapshot
      .mockImplementationOnce(() => new Promise(() => undefined))
      .mockResolvedValue(snapshot(failedBatch));
    const { container } = render(<HistoryPage />);

    expect(container.querySelector(".status-badge.status-running")).toBeInTheDocument();
    act(() => applyEvent("task-progress", { task_id: "live", ecosystem: "gem", name: "rake@13.0.6", operation: "update", sequence: 1, emitted_at: 1, status: "failed", error: "command_failed", completed: 1, total: 1 }));

    await waitFor(() => expect(container.querySelector(".status-badge.status-failed")).toBeInTheDocument());
    expect(await screen.findByRole("button", { name: /retry|重试/i })).toBeInTheDocument();
  });
});
