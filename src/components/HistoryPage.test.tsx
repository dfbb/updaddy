import { describe, expect, it } from "vitest";
import type { OperationBatch, PackageTask } from "../types";
import { batchCounts, taskMatches } from "./HistoryPage";

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
});
