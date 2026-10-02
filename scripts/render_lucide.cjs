// Преобразование официальных SVG в изображения для встроенных кнопок Win32.
const fs = require('fs');
const path = require('path');
const sharp = require(process.env.ASTRA_SHARP || 'sharp');
const root = path.resolve(__dirname, '../assets/lucide');
(async () => {
  for (const name of JSON.parse(fs.readFileSync(path.join(root, 'source.json'))).icons) {
    await sharp(path.join(root, name + '.svg'), {density: 384}).resize(80, 80).png().toFile(path.join(root, name + '.png'));
  }
})();
