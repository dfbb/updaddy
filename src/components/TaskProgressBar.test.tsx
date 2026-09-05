import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import TaskProgressBar from "./TaskProgressBar";

describe("TaskProgressBar", () => {
  it("shows the real operation, package, and an honest indeterminate status", () => {
    render(<TaskProgressBar ecosystem="homebrew" tasks={[
      { task_id: "update", ecosystem: "homebrew", name: "openssl@3", operation: "update", status: "running", completed: 25_000_000, total: 100_000_000, eta_seconds: 75 },
    ]} />);

    expect(screen.getByText(/^(updating|更新中)$/i)).toBeInTheDocument();
    expect(screen.getAllByText(/openssl@3/i).length).toBeGreaterThan(0);
    expect(screen.getByText(/25\.0 MB.*100\.0 MB.*25%.*1:15/i)).toBeInTheDocument();
    expect(screen.queryByText(/ecosystem scan|生态扫描/i)).not.toBeInTheDocument();
  });

  it("keeps cancel available and targets the running task before queued tasks", async () => {
    const cancel = vi.fn();
    render(<TaskProgressBar ecosystem="npm" onCancel={cancel} tasks={[
      { task_id: "queued", ecosystem: "npm", name: "queued", operation: "update", status: "pending" },
      { task_id: "running", ecosystem: "npm", name: "running", operation: "update", status: "running" },
    ]} />);

    const button = screen.getByRole("button", { name: /cancel|取消/i });
    expect(button).toBeEnabled();
    await userEvent.click(button);
    expect(cancel).toHaveBeenCalledWith("running");
  });
});
