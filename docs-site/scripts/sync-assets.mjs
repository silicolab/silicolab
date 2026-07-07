// Copies canonical repo assets into git-ignored paths the site build expects.
// Runs automatically via the predev/prebuild hooks in package.json.
import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const docsSite = dirname(dirname(fileURLToPath(import.meta.url)));
const repo = dirname(docsSite);

const assets = [
  ['assets/brand/wordmark-light.svg', 'src/assets/wordmark-light.svg'],
  ['assets/brand/wordmark-dark.svg', 'src/assets/wordmark-dark.svg'],
  ['docs/images/main-window.png', 'src/assets/main-window.png'],
  ['assets/icon/hicolor/256x256/apps/silicolab.png', 'public/favicon.png'],
];

for (const [src, dest] of assets) {
  const to = join(docsSite, dest);
  mkdirSync(dirname(to), { recursive: true });
  copyFileSync(join(repo, src), to);
}
