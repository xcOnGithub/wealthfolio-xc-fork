import { BasisError } from "./opening-basis";

export const MAX_BASIS_FILE_BYTES = 10 * 1024 * 1024;
export const MAX_BASIS_TEXT = 1_000_000;

interface TextItem {
  str: string;
  transform: number[];
  width: number;
}

/** Preserve column gaps as tabs, leaving the extracted table editable for review. */
export function pdfTableText(items: TextItem[]): string {
  if (items.length > 50000) throw new BasisError("size");
  const lines: { y: number; cells: TextItem[] }[] = [];
  const buckets = new Map<number, (typeof lines)[number]>();
  for (const item of items) {
    if (!item.str.trim()) continue;
    const y = item.transform[5];
    const bucket = Math.floor(y / 2);
    const line = [bucket, bucket - 1, bucket + 1]
      .map((key) => buckets.get(key))
      .find((candidate) => candidate && Math.abs(candidate.y - y) < 2);
    if (line) line.cells.push(item);
    else {
      const created = { y, cells: [item] };
      lines.push(created);
      buckets.set(bucket, created);
    }
  }
  return lines
    .sort((a, b) => b.y - a.y)
    .map(({ cells }) => {
      cells.sort((a, b) => a.transform[4] - b.transform[4]);
      return cells
        .map((cell, i) => {
          const previous = cells[i - 1];
          const gap = previous ? cell.transform[4] - previous.transform[4] - previous.width : 0;
          return `${i ? (gap > 10 ? "\t" : " ") : ""}${cell.str.replace(/[\t\r\n]/g, " ")}`;
        })
        .join("");
    })
    .join("\n");
}

export async function readBasisFile(file: File): Promise<string> {
  if (!file.size || file.size > MAX_BASIS_FILE_BYTES) throw new BasisError("size");
  if (/\.csv$/i.test(file.name)) {
    const text = await file.text();
    if (text.length > MAX_BASIS_TEXT) throw new BasisError("size");
    return text;
  }
  if (!/\.pdf$/i.test(file.name)) throw new BasisError("file");
  const data = new Uint8Array(await file.arrayBuffer());
  if (new TextDecoder().decode(data.slice(0, 5)) !== "%PDF-") throw new BasisError("file");
  // Loaded only for PDFs. The worker is bundled locally: no CDN or document upload.
  const [pdf, worker] = await Promise.all([
    import("pdfjs-dist"),
    import("pdfjs-dist/build/pdf.worker.min.mjs?url"),
  ]);
  pdf.GlobalWorkerOptions.workerSrc = worker.default;
  const task = pdf.getDocument({
    data,
    disableFontFace: true,
    useSystemFonts: false,
    useWorkerFetch: false,
    stopAtErrors: true,
  });
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const read = async () => {
      const doc = await task.promise;
      if (doc.numPages > 100) throw new BasisError("size");
      const pages: string[] = [];
      let length = 0;
      for (let index = 1; index <= doc.numPages; index++) {
        const page = await doc.getPage(index);
        const content = await page.getTextContent();
        const text = pdfTableText(
          content.items.filter((item): item is typeof item & TextItem => "str" in item),
        );
        length += text.length;
        if (length > MAX_BASIS_TEXT) throw new BasisError("size");
        pages.push(text);
        page.cleanup();
      }
      const text = pages.join("\n");
      if (!text.trim()) throw new BasisError("scanned");
      return text;
    };
    return await Promise.race([
      read(),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new BasisError("pdf")), 30000);
      }),
    ]);
  } catch (error) {
    if (error instanceof BasisError) throw error;
    throw new BasisError("pdf");
  } finally {
    clearTimeout(timer);
    await task.destroy();
  }
}
