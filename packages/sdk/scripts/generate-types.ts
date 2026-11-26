// Generates src/hwf.ts, the HWF v1 document types, from the published
// schema (schemas/hwf/v1.json). Run `pnpm generate` after the schema grows.
import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { generate } from './types.ts';

const schemaPath = fileURLToPath(new URL('../../../schemas/hwf/v1.json', import.meta.url));
const outPath = fileURLToPath(new URL('../src/hwf.ts', import.meta.url));
const schema = JSON.parse(await readFile(schemaPath, 'utf8')) as Record<string, unknown>;
await writeFile(outPath, await generate(schema));
console.log(`wrote ${outPath}`);
