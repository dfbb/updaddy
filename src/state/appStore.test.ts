import { describe, expect, it } from "vitest";
import { applyEvent, applyEvents, applySnapshot, getAppState, setSettings, defaultSettings } from "./appStore";

describe("app store", () => {
  it("keeps operations disabled while a task is active", () => {
    applySnapshot({ workers: {}, packages: [], logs: [], active_tasks: 1, tasks: [{ task_id: "t1", ecosystem: "npm", name: "x", operation: "update", status: "running" }] });
    expect(getAppState().operationsDisabled).toBe(true);
    applyEvent("task-progress", { task_id: "t1", ecosystem: "npm", sequence: 1, emitted_at: 0, status: "succeeded" });
    expect(getAppState().operationsDisabled).toBe(false);
  });

  it("uses task metadata from a progress event without requiring a snapshot race", () => {
    applySnapshot({ workers: {}, packages: [], logs: [], active_tasks: 0, tasks: [] });
    applyEvent("task-progress", { task_id: "update", ecosystem: "npm", name: "eslint", operation: "update", sequence: 1, emitted_at: 0, status: "running", completed: 25, total: 100, eta_seconds: 75 });
    expect(getAppState().tasks).toEqual([expect.objectContaining({ task_id: "update", name: "eslint", operation: "update", completed: 25, total: 100, eta_seconds: 75 })]);

    applyEvent("task-progress", { task_id: "update", ecosystem: "npm", name: "eslint", operation: "update", sequence: 2, emitted_at: 1, status: "succeeded", completed: 1, total: 1 });
    expect(getAppState().tasks).toEqual([]);
  });

  it("hides missing tools but keeps temporarily unavailable ecosystems visible", () => {
    setSettings(defaultSettings);
    applySnapshot({ workers: { npm: "missing", gem: "unavailable" }, packages: [], logs: [], active_tasks: 0, tasks: [] });
    expect(getAppState().visibleEcosystems).not.toContain("npm");
    expect(getAppState().visibleEcosystems).toContain("gem");
  });

  it("keeps only active user tasks and applies a burst of package events together", () => {
    applySnapshot({
      workers: { homebrew: "running" },
      packages: [],
      logs: [],
      active_tasks: 1,
      tasks: [
        { task_id: "scan", ecosystem: "homebrew", name: "*", operation: "scan", status: "running" },
        { task_id: "disk", ecosystem: "homebrew", name: "openssl", operation: "measure_disk", status: "running" },
        { task_id: "old", ecosystem: "homebrew", name: "curl", operation: "update", status: "succeeded" },
      ],
    });
    expect(getAppState().tasks.map((task) => task.task_id)).toEqual(["scan"]);

    applyEvents(Array.from({ length: 500 }, (_, index) => ["package-changed", {
      ecosystem: "homebrew",
      sequence: index + 1,
      emitted_at: 0,
      package: { id: `homebrew:${index}`, ecosystem: "homebrew", resource_kind: "formula", name: `package-${index}`, update_available: false },
    }] as const));

    expect(getAppState().packages).toHaveLength(500);
    expect(getAppState().tasks.map((task) => task.task_id)).toEqual(["scan"]);
  });
});
