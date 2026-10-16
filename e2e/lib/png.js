// Small PNG pictures for attachments in tests and screenshots, drawn here (no files in the
// repository): a soft landscape of sky, hills and water in the given colors.

import zlib from "node:zlib";

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(zlib.crc32(td) >>> 0);
  return Buffer.concat([len, td, crc]);
}

const mix = (a, b, t) => a.map((v, i) => Math.round(v + (b[i] - v) * t));

/** A landscape picture, `width` x `height`; `palette`: sky top, sky bottom, hills, water. */
export function landscape(width = 480, height = 320, palette = [[156, 190, 222], [240, 214, 186], [92, 120, 98], [104, 142, 170]]) {
  const [skyTop, skyBottom, hills, water] = palette;
  const raw = Buffer.alloc((width * 3 + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (width * 3 + 1)] = 0;
    for (let x = 0; x < width; x++) {
      const ridge = height * 0.55 + Math.sin(x / 47) * 18 + Math.sin(x / 13 + 1) * 5;
      const shore = height * 0.78;
      let c;
      if (y > shore) c = mix(water, [40, 70, 96], (y - shore) / (height - shore));
      else if (y > ridge) c = mix(hills, [52, 74, 60], (y - ridge) / (shore - ridge));
      else c = mix(skyTop, skyBottom, y / ridge);
      const o = y * (width * 3 + 1) + 1 + x * 3;
      raw[o] = c[0];
      raw[o + 1] = c[1];
      raw[o + 2] = c[2];
    }
  }
  const head = Buffer.alloc(13);
  head.writeUInt32BE(width, 0);
  head.writeUInt32BE(height, 4);
  head[8] = 8;
  head[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", head), chunk("IDAT", zlib.deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}

/** A one-page PDF with a line of text, about `size` bytes (filled up with a comment, as real
 *  invoices with fonts and logos are larger). */
export function pdf(text, size = 0) {
  const stream = `BT /F1 18 Tf 72 720 Td (${text.replace(/[()\\]/g, "\\$&")}) Tj ET`;
  const objs = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    `<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`,
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
  ];
  let out = "%PDF-1.4\n";
  const offsets = [];
  objs.forEach((o, i) => {
    offsets.push(out.length);
    out += `${i + 1} 0 obj\n${o}\nendobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n${offsets.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("")}`;
  out += `trailer\n<< /Size ${objs.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  while (out.length < size) out += `% ${"0".repeat(76)}\n`;
  return Buffer.from(out, "latin1");
}
