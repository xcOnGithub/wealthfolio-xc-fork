import { describe, expect, it, vi } from "vitest";
import {
  basisDate,
  MAX_BASIS_BATCH_BYTES,
  MAX_BASIS_FILE_BYTES,
  pdfTableText,
  readBasisFile,
  validateBasisFiles,
} from "./statement-pdf";

const { getDocument } = vi.hoisted(() => ({ getDocument: vi.fn() }));
vi.mock("pdfjs-dist", () => ({ getDocument, GlobalWorkerOptions: {} }));
vi.mock("pdfjs-dist/build/pdf.worker.min.mjs?url", () => ({ default: "/local-pdf-worker.mjs" }));
const fakePdf = () =>
  ({
    size: 10,
    name: "statement.pdf",
    arrayBuffer: async () => new TextEncoder().encode("%PDF-1.4").buffer,
  }) as File;

describe("statement extraction", () => {
  it("cleans up PDF workers for encrypted files, empty scans and excessive pages", async () => {
    for (const [promise, code] of [
      [() => Promise.reject(new Error("password includes private data")), "pdf"],
      [() => Promise.resolve({ numPages: 101 }), "size"],
      [
        () =>
          Promise.resolve({
            numPages: 1,
            getPage: async () => ({
              getTextContent: async () => ({ items: [] }),
              cleanup: vi.fn(),
            }),
          }),
        "scanned",
      ],
    ] as const) {
      const destroy = vi.fn();
      getDocument.mockReturnValueOnce({
        get promise() {
          return promise();
        },
        destroy,
      });
      await expect(readBasisFile(fakePdf())).rejects.toThrow(code);
      expect(destroy).toHaveBeenCalledOnce();
    }
  });
  it("destroys a stalled PDF task after the deadline", async () => {
    vi.useFakeTimers();
    const destroy = vi.fn();
    getDocument.mockReturnValueOnce({ promise: new Promise(() => {}), destroy });
    try {
      const reading = readBasisFile(fakePdf());
      const rejection = expect(reading).rejects.toThrow("pdf");
      await vi.advanceTimersByTimeAsync(30001);
      await rejection;
      expect(destroy).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });
  it("orders PDF rows and columns and preserves gaps for the existing TSV parser", () => {
    const item = (str: string, x: number, y: number, width: number) => ({
      str,
      transform: [1, 0, 0, 1, x, y],
      width,
    });
    expect(
      pdfTableText([
        item("2", 180, 680, 10),
        item("ABC", 30, 680, 25),
        item("25.00", 260, 680, 35),
        item("Symbol", 30, 700, 40),
        item("Quantity", 180, 700, 45),
        item("Cost", 260, 700, 25),
        item("basis", 290, 700, 28),
      ]),
    ).toBe("Symbol\tQuantity\tCost basis\nABC\t2\t25.00");
  });
  it("does not execute or interpret document text", () => {
    expect(
      pdfTableText([
        { str: "<script>secret</script>\ntext", transform: [1, 0, 0, 1, 0, 0], width: 10 },
      ]),
    ).toBe("<script>secret</script> text");
  });
  it("rejects oversized, empty, non-PDF and invalid-signature files before loading PDF code", async () => {
    await expect(readBasisFile({ size: MAX_BASIS_FILE_BYTES + 1 } as File)).rejects.toThrow("size");
    await expect(readBasisFile(new File([], "empty.pdf"))).rejects.toThrow("size");
    await expect(readBasisFile(new File(["Symbol,Quantity\nABC,2"], "DATA.CSV"))).rejects.toThrow(
      "file",
    );
    await expect(readBasisFile(new File(["data"], "data.exe"))).rejects.toThrow("file");
    const fake = {
      size: 5,
      name: "bad.pdf",
      arrayBuffer: vi.fn().mockResolvedValue(new TextEncoder().encode("bad!!").buffer),
    } as unknown as File;
    await expect(readBasisFile(fake)).rejects.toThrow("file");
  });
  it("validates ISO calendar dates", () => {
    expect(basisDate("2024-02-29")).toBe("2024-02-29");
    for (const date of ["2025-02-29", "2025-13-01", "05/12/2025", "invalid"]) {
      expect(() => basisDate(date)).toThrow("date");
    }
  });
  it("enforces five-file and aggregate byte limits", () => {
    const files = Array.from({ length: 6 }, () => new File(["x"], "statement.pdf"));
    expect(() => validateBasisFiles([])).toThrow("batchCount");
    expect(() => validateBasisFiles(files)).toThrow("batchCount");
    expect(() =>
      validateBasisFiles(Array.from({ length: 3 }, () => ({ size: 10 * 1024 * 1024 }) as File)),
    ).toThrow("batchSize");
    expect(MAX_BASIS_BATCH_BYTES).toBe(25 * 1024 * 1024);
    expect(() => validateBasisFiles(files.slice(0, 5))).not.toThrow();
  });
});
