// ELK 0.12.0 indexes optional children without first removing undefined.
// Keep strict dependency type checking; change only this declaration expression.
import { readFile, writeFile } from 'node:fs/promises';

const packagePath = new URL('./node_modules/elkjs/package.json', import.meta.url);
const declarationPath = new URL('./node_modules/elkjs/lib/elk-api.d.ts', import.meta.url);
const version = JSON.parse(await readFile(packagePath, 'utf8')).version;
if (version !== '0.12.0') throw new Error(`Review the ELK type patch for version ${version}`);
const original = "T['children'][number]";
const corrected = "NonNullable<T['children']>[number]";
const source = await readFile(declarationPath, 'utf8');
if (source.split(corrected).length === 2 && !source.includes(original)) {
  // Already prepared; no file write during verification.
} else if (process.argv[2] === '--apply' && source.split(original).length === 2 && !source.includes(corrected)) {
  await writeFile(declarationPath, source.replace(original, corrected));
} else {
  throw new Error('ELK declarations are unprepared or changed; run npm run prepare:types after npm ci and review unexpected changes');
}
