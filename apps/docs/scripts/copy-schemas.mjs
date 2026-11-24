// Publishes the HWF JSON Schema with the site, at /schemas/hwf/v1.json
// (the URL its $id names), from the copy the API tests keep current.
import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const target = join(here, '..', '.vitepress', 'dist', 'schemas', 'hwf');
mkdirSync(target, { recursive: true });
copyFileSync(join(here, '..', '..', '..', 'schemas', 'hwf', 'v1.json'), join(target, 'v1.json'));
console.log('copied schemas/hwf/v1.json');
