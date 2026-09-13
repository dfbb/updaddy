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

  it("does not leave a completed Homebrew sub-download displayed as overall progress", () => {
    render(<TaskProgressBar ecosystem="homebrew" tasks={[
      { task_id: "update", ecosystem: "homebrew", name: "cask:chatgpt", operation: "update", status: "running", completed: 0, total: 0, phase: "processing" },
    ]} />);

    expect(screen.getByText(/Homebrew/)).toBeInTheDocument();
    expect(screen.queryByText("100%")).not.toBeInTheDocument();
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
  });

  it("shows live downloaded bytes without inventing a total, percentage, or ETA", () => {
    render(<TaskProgressBar ecosystem="homebrew" tasks={[
      { task_id: "update", ecosystem: "homebrew", name: "cask:chatgpt", operation: "update", status: "running", completed: 476_200_000, total: 0, phase: "downloading-unknown-total" },
    ]} />);

    expect(screen.getByText(/476\.2 MB/)).toBeInTheDocument();
    expect(screen.queryByText(/%|remaining|剩余/)).not.toBeInTheDocument();
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
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

  it("shows a backend error when cancellation cannot be applied", async () => {
    const cancel = vi.fn().mockRejectedValue(new Error("task is not active"));
    render(<TaskProgressBar ecosystem="npm" onCancel={cancel} tasks={[
      { task_id: "stale", ecosystem: "npm", name: "stale", operation: "update", status: "pending" },
    ]} />);

    await userEvent.click(screen.getByRole("button", { name: /cancel|取消/i }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/task is not active/i);
  });

  it("opens a live console with the current task command and output", async () => {
    render(<TaskProgressBar ecosystem="homebrew" tasks={[
      { task_id: "docker", ecosystem: "homebrew", name: "cask:docker-desktop", operation: "update", status: "running" },
    ]} logs={[
      { task_id: "docker", emitted_at: 1, stream: "command", message: "$ brew upgrade --yes --cask docker-desktop" },
      { task_id: "docker", emitted_at: 2, stream: "stdout", message: "==> Downloading Docker Desktop" },
      { task_id: "other", emitted_at: 2, stream: "stdout", message: "other task" },
    ]} />);

    await userEvent.click(screen.getByRole("button", { name: /view log|查看日志/i }));

    expect(screen.getByRole("dialog")).toHaveTextContent("$ brew upgrade --yes --cask docker-desktop");
    expect(screen.getByRole("dialog")).toHaveTextContent("==> Downloading Docker Desktop");
    expect(screen.getByRole("dialog")).not.toHaveTextContent("other task");
  });
});
