import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import TaskProgressBar from "./TaskProgressBar";

describe("TaskProgressBar", () => {
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
