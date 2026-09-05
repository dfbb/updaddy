import { describe, expect, it } from "vitest";
import { parseSchedule, serializeSchedule } from "./SettingsPage";

describe("SettingsPage schedule", () => {
  it("round trips daily and weekly schedules", () => {
    expect(serializeSchedule(parseSchedule("09:30"))).toBe("09:30");
    expect(serializeSchedule(parseSchedule("weekly:friday 18:45"))).toBe("weekly:friday 18:45");
  });

  it("keeps a disabled schedule unset", () => {
    expect(serializeSchedule(parseSchedule(undefined))).toBeUndefined();
  });
});
