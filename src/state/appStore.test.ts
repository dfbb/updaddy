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

  it("updates the matching history batch when task status changes", () => {
    applySnapshot({
      workers: {}, packages: [], logs: [], active_tasks: 1,
      tasks: [{ task_id: "history-task", ecosystem: "gem", name: "rake@13.0.6", operation: "update", status: "running" }],
      batches: [{ batch_id: "batch", ecosystem: "gem", created_at: 1, tasks: [{ task_id: "history-task", ecosystem: "gem", name: "rake@13.0.6", operation: "update", status: "running" }] }],
    });

    applyEvent("task-progress", { task_id: "history-task", ecosystem: "gem", name: "rake@13.0.6", operation: "update", sequence: 1, emitted_at: 1, status: "failed", error: "command_failed", completed: 1, total: 1 });

    expect(getAppState().batches[0].tasks[0]).toMatchObject({ status: "failed", error: "command_failed" });
    expect(getAppState().historyRevision).toBeGreaterThan(0);
  });

  it("clears completed download progress when Homebrew enters processing", () => {
    applySnapshot({ workers: {}, packages: [], logs: [], active_tasks: 1, tasks: [{ task_id: "brew", ecosystem: "homebrew", name: "cask:chatgpt", operation: "update", status: "running" }] });
    applyEvent("task-progress", { task_id: "brew", ecosystem: "homebrew", sequence: 1, emitted_at: 0, status: "running", completed: 15_500_000, total: 15_500_000, eta_seconds: 0 });
    applyEvent("task-progress", { task_id: "brew", ecosystem: "homebrew", sequence: 2, emitted_at: 1, status: "running", completed: 0, total: 0, eta_seconds: null, phase: "processing" });

    expect(getAppState().tasks[0]).toMatchObject({ completed: 0, total: 0, eta_seconds: undefined, phase: "processing" });
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

  it("removes an uninstalled package immediately", () => {
    applySnapshot({ workers: {}, packages: [{ id: "npm:gone", ecosystem: "npm", resource_kind: "package", name: "gone", update_available: true }], logs: [], active_tasks: 0, tasks: [] });

    applyEvent("package-removed", { ecosystem: "npm", sequence: 1, emitted_at: 0, package_id: "npm:gone" });

    expect(getAppState().packages).toEqual([]);
    expect(getAppState().totalUpdates).toBe(0);
  });

  it("keeps live command output scoped to its task", () => {
    applySnapshot({ workers: {}, packages: [], logs: [], active_tasks: 0, tasks: [] });
    applyEvent("log-entry", { ecosystem: "homebrew", sequence: 1, emitted_at: 1, task_id: "docker", entry: { emitted_at: 1, stream: "stdout", message: "download output" } });

    expect(getAppState().logs).toContainEqual(expect.objectContaining({ task_id: "docker", message: "download output" }));
  });
});
