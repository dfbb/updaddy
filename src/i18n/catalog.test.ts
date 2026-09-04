import { describe, expect, it } from "vitest";
import { createTranslator } from "./catalog";
describe("translator", () => { it("falls back to English for a missing locale key", () => { const translator = createTranslator({ zh_CN: { task: {} }, en: { task: { cancel: "Cancel" } } }); expect(translator.t("task.cancel", "zh_CN")).toBe("Cancel"); }); });
