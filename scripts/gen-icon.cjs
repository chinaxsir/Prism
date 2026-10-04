// 生成最小有效 icon.ico（内嵌 256x256 PNG，靛蓝色块 + 白色 P 字形）
// 仅用于本地调试；正式发布前替换为品牌设计图标（pnpm tauri icon path/to/logo.png）
const fs = require("fs");
const zlib = require("zlib");
const path = require("path");

const SIZE = 256;

// ---- 构造 RGBA 像素 ----
const px = Buffer.alloc(SIZE * SIZE * 4);

function setPx(x, y, r, g, b, a = 255) {
  const i = (y * SIZE + x) * 4;
  px[i] = r;
  px[i + 1] = g;
  px[i + 2] = b;
  px[i + 3] = a;
}

// 圆角矩形底
const R = 56;
for (let y = 0; y < SIZE; y++) {
  for (let x = 0; x < SIZE; x++) {
    const cx = Math.min(Math.max(x, R), SIZE - 1 - R);
    const cy = Math.min(Math.max(y, R), SIZE - 1 - R);
    const d = Math.hypot(x - cx, y - cy);
    if (d <= R) setPx(x, y, 99, 102, 241); // indigo-500
  }
}

// 简单 7x9 点阵字母 P，放大 16 倍，居中
const GLYPH = [
  "1111110",
  "1000010",
  "1000010",
  "1000010",
  "1111110",
  "1000000",
  "1000000",
  "1000000",
  "1000000",
];
const SCALE = 16;
const gw = 7 * SCALE;
const gh = 9 * SCALE;
const ox = Math.floor((SIZE - gw) / 2);
const oy = Math.floor((SIZE - gh) / 2);

for (let gy = 0; gy < 9; gy++) {
  for (let gx = 0; gx < 7; gx++) {
    if (GLYPH[gy][gx] !== "1") continue;
    for (let sy = 0; sy < SCALE; sy++) {
      for (let sx = 0; sx < SCALE; sx++) {
        setPx(ox + gx * SCALE + sx, oy + gy * SCALE + sy, 255, 255, 255);
      }
    }
  }
}

// ---- 编码 PNG（truecolor + alpha, 8-bit）----
function crc32(buf) {
  let c = ~0;
  for (let i = 0; i < buf.length; i++) {
    c ^= buf[i];
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // color type RGBA
// 其余默认 0

// 每行前置 filter byte 0
const raw = Buffer.alloc(SIZE * (SIZE * 4 + 1));
for (let y = 0; y < SIZE; y++) {
  px.copy(raw, y * (SIZE * 4 + 1) + 1, y * SIZE * 4, (y + 1) * SIZE * 4);
}
const idat = zlib.deflateSync(raw, { level: 9 });

const png = Buffer.concat([
  sig,
  chunk("IHDR", ihdr),
  chunk("IDAT", idat),
  chunk("IEND", Buffer.alloc(0)),
]);

// ---- 包装为 ICO（单张 PNG 条目，Vista+ 支持）----
const header = Buffer.alloc(6);
header.writeUInt16LE(0, 0); // reserved
header.writeUInt16LE(1, 2); // type: icon
header.writeUInt16LE(1, 4); // count

const entry = Buffer.alloc(16);
entry[0] = 0; // 256 → 0
entry[1] = 0;
entry[2] = 0; // palette
entry[3] = 0; // reserved
entry.writeUInt16LE(1, 4); // color planes
entry.writeUInt16LE(32, 6); // bpp
entry.writeUInt32LE(png.length, 8);
entry.writeUInt32LE(6 + 16, 12); // offset

const outDir = path.join(__dirname, "..", "src-tauri", "icons");
fs.mkdirSync(outDir, { recursive: true });
fs.writeFileSync(path.join(outDir, "icon.ico"), Buffer.concat([header, entry, png]));
console.log("icon.ico written:", png.length, "bytes PNG payload");
