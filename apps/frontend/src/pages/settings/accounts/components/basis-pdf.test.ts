import { describe, expect, it, vi } from "vitest";
import { MAX_BASIS_FILE_BYTES, pdfTableText, readBasisFile } from "./basis-pdf";

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
  it("rejects oversized, empty, invalid signature and unsupported files before loading PDF code", async () => {
    await expect(readBasisFile({ size: MAX_BASIS_FILE_BYTES + 1 } as File)).rejects.toThrow("size");
    await expect(readBasisFile(new File([], "empty.csv"))).rejects.toThrow("size");
    await expect(readBasisFile(new File(["data"], "data.exe"))).rejects.toThrow("file");
    const fake = {
      size: 5,
      name: "bad.pdf",
      arrayBuffer: vi.fn().mockResolvedValue(new TextEncoder().encode("bad!!").buffer),
    } as unknown as File;
    await expect(readBasisFile(fake)).rejects.toThrow("file");
  });
  it("reads CSV without PDF processing and enforces the text limit", async () => {
    const file = {
      size: 100,
      name: "DATA.CSV",
      text: vi.fn().mockResolvedValue("Symbol,Quantity\nABC,2"),
    } as unknown as File;
    expect(await readBasisFile(file)).toBe("Symbol,Quantity\nABC,2");
    vi.mocked(file.text).mockResolvedValue("a".repeat(1_000_001));
    await expect(readBasisFile(file)).rejects.toThrow("size");
  });
});
