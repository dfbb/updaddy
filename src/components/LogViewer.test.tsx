import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import LogViewer from "./LogViewer";

describe("LogViewer", () => {
  it("renders command output as plain text", () => {
    render(<LogViewer entries={[{ emitted_at: 1, stream: "stderr", message: "<script>alert(1)</script>" }]} />);
    expect(screen.getByText("<script>alert(1)</script>")).toBeInTheDocument();
    expect(document.querySelector("script")).toBeNull();
  });
});
