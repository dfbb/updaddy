import { describe, expect, it } from "vitest";
import { applyEvent, applySnapshot, getAppState, setSettings, defaultSettings } from "./appStore";

describe("app store", () => {
  it("keeps operations disabled while a task is active", () => {
    applySnapshot({ workers: {}, packages: [], logs: [], active_tasks: 1, tasks: [{ task_id: "t1", ecosystem: "npm", name: "x", operation: "update", status: "running" }] });
    expect(getAppState().operationsDisabled).toBe(true);
    applyEvent("task-progress", { task_id: "t1", ecosystem: "npm", sequence: 1, emitted_at: 0, status: "succeeded" });
    expect(getAppState().operationsDisabled).toBe(false);
  });

  it("hides missing tools but keeps temporarily unavailable ecosystems visible", () => {
    setSettings(defaultSettings);
    applySnapshot({ workers: { npm: "missing", gem: "unavailable" }, packages: [], logs: [], active_tasks: 0, tasks: [] });
    expect(getAppState().visibleEcosystems).not.toContain("npm");
    expect(getAppState().visibleEcosystems).toContain("gem");
  });
});
