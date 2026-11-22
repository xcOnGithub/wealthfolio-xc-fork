/** Calendar month in the selected profile, independent of the browser timezone. */
export function previousFamilyMonth(now: Date, timezone = "UTC") {
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone: timezone.trim() || "UTC",
    year: "numeric",
    month: "numeric",
  }).formatToParts(now);
  const year = Number(parts.find((p) => p.type === "year")?.value);
  const month = Number(parts.find((p) => p.type === "month")?.value);
  return new Date(Date.UTC(year, month - 1, 0)).toISOString().slice(0, 7);
}
