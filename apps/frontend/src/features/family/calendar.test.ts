import { expect, it } from "vitest";
import { previousFamilyMonth } from "./calendar";
it("uses the profile calendar at month and year boundaries", () => {
  const now = new Date("2026-01-01T00:30:00Z");
  expect(previousFamilyMonth(now, "America/Los_Angeles")).toBe("2025-11");
  expect(previousFamilyMonth(now, "Asia/Tokyo")).toBe("2025-12");
  expect(previousFamilyMonth(now, "")).toBe("2025-12");
  expect(previousFamilyMonth(now, "  ")).toBe("2025-12");
  expect(previousFamilyMonth(new Date("2024-03-01T12:00:00Z"))).toBe("2024-02");
});
