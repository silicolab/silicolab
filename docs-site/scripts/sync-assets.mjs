// Copies canonical repo assets into git-ignored paths the site build expects.
// Runs automatically via the predev/prebuild hooks in package.json.
import { copyFileSync, mkdirSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const docsSite = dirname(dirname(fileURLToPath(import.meta.url)));
const repo = dirname(docsSite);

const assets = [
  ['branding/wordmark-light.svg', 'src/assets/wordmark-light.svg'],
  ['branding/wordmark-dark.svg', 'src/assets/wordmark-dark.svg'],
  ['assets/icon/hicolor/256x256/apps/silicolab.png', 'public/favicon.png'],
  ['docs-site/manual/samples/6a5j-two-model-ui-fixture.pdb', 'public/samples/6a5j-two-model-ui-fixture.pdb'],
  ['docs-site/manual/samples/argon.xyz', 'public/samples/argon.xyz'],
  ['compute-core/assets/ubl/ubiquitin.pdb', 'public/samples/ubiquitin.pdb'],
];

// Prevent deleted samples from lingering in published output.
rmSync(join(docsSite, 'public/samples'), { recursive: true, force: true });

for (const [src, dest] of assets) {
  const to = join(docsSite, dest);
  mkdirSync(dirname(to), { recursive: true });
  copyFileSync(join(repo, src), to);
}
