// scripts/generate_icons.mjs
// 将 SVG 转换为 Tauri 所需的所有 icon 尺寸
import sharp from 'sharp';
import path from 'path';
import { fileURLToPath } from 'url';
import fs from 'fs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const svgPath = path.join(__dirname, 'icon_source.svg');
const iconsDir = path.join(__dirname, '..', 'src-tauri', 'icons');

const svgBuffer = fs.readFileSync(svgPath);

const sizes = [
  { name: '32x32.png', size: 32 },
  { name: '64x64.png', size: 64 },
  { name: '128x128.png', size: 128 },
  { name: '128x128@2x.png', size: 256 },
  { name: 'icon.png', size: 512 },
  // Android
  { name: 'android/mipmap-mdpi/ic_launcher.png', size: 48 },
  { name: 'android/mipmap-hdpi/ic_launcher.png', size: 72 },
  { name: 'android/mipmap-xhdpi/ic_launcher.png', size: 96 },
  { name: 'android/mipmap-xxhdpi/ic_launcher.png', size: 144 },
  { name: 'android/mipmap-xxxhdpi/ic_launcher.png', size: 192 },
  // Windows Store
  { name: 'Square30x30Logo.png', size: 30 },
  { name: 'Square44x44Logo.png', size: 44 },
  { name: 'Square71x71Logo.png', size: 71 },
  { name: 'Square89x89Logo.png', size: 89 },
  { name: 'Square107x107Logo.png', size: 107 },
  { name: 'Square142x142Logo.png', size: 142 },
  { name: 'Square150x150Logo.png', size: 150 },
  { name: 'Square284x284Logo.png', size: 284 },
  { name: 'Square310x310Logo.png', size: 310 },
  { name: 'StoreLogo.png', size: 50 },
];

async function generate() {
  for (const { name, size } of sizes) {
    const outPath = path.join(iconsDir, name);
    fs.mkdirSync(path.dirname(outPath), { recursive: true });
    await sharp(svgBuffer)
      .resize(size, size)
      .png()
      .toFile(outPath);
    console.log(`✓ ${name} (${size}x${size})`);
  }

  // Generate .ico (multi-size: 16, 32, 48, 256)
  // sharp doesn't support .ico natively, use 256px PNG and rename as workaround
  // Better: use the 256px as source for ICO via buffer concat
  console.log('\nGenerating icon.ico ...');
  await generateIco();

  // Generate icon.icns (macOS) - use 512px as base
  // Tauri's bundler can handle .png as fallback for macOS
  // Copy 512px as icon.icns placeholder (real .icns needs iconutil on macOS)
  const icnsPath = path.join(iconsDir, 'icon.icns');
  const png512 = await sharp(svgBuffer).resize(512, 512).png().toBuffer();
  fs.writeFileSync(icnsPath, png512);
  console.log('✓ icon.icns (512px PNG fallback - rebuild on macOS for native .icns)');

  console.log('\nAll icons generated successfully!');
}

async function generateIco() {
  // ICO format: file header + directory + image data
  // Support sizes: 16, 32, 48, 256
  const icoSizes = [16, 32, 48, 256];
  const images = await Promise.all(
    icoSizes.map(s =>
      sharp(svgBuffer)
        .resize(s, s)
        .png()
        .toBuffer()
    )
  );

  const ICO_HEADER_SIZE = 6;
  const ICO_DIR_ENTRY_SIZE = 16;
  const headerSize = ICO_HEADER_SIZE + ICO_DIR_ENTRY_SIZE * icoSizes.length;

  // Calculate offsets
  let offset = headerSize;
  const offsets = images.map(buf => {
    const o = offset;
    offset += buf.length;
    return o;
  });

  // ICO file header
  const header = Buffer.alloc(ICO_HEADER_SIZE);
  header.writeUInt16LE(0, 0);  // reserved
  header.writeUInt16LE(1, 2);  // type: 1 = ICO
  header.writeUInt16LE(icoSizes.length, 4); // image count

  // Directory entries
  const dirEntries = images.map((buf, i) => {
    const entry = Buffer.alloc(ICO_DIR_ENTRY_SIZE);
    const s = icoSizes[i];
    entry.writeUInt8(s >= 256 ? 0 : s, 0);  // width (0 = 256)
    entry.writeUInt8(s >= 256 ? 0 : s, 1);  // height (0 = 256)
    entry.writeUInt8(0, 2);  // color count
    entry.writeUInt8(0, 3);  // reserved
    entry.writeUInt16LE(1, 4);  // color planes
    entry.writeUInt16LE(32, 6); // bits per pixel
    entry.writeUInt32LE(buf.length, 8);  // size of image data
    entry.writeUInt32LE(offsets[i], 12); // offset of image data
    return entry;
  });

  const icoBuffer = Buffer.concat([header, ...dirEntries, ...images]);
  const icoPath = path.join(iconsDir, 'icon.ico');
  fs.writeFileSync(icoPath, icoBuffer);
  console.log('✓ icon.ico (16/32/48/256px)');
}

generate().catch(console.error);
