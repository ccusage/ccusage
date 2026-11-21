import { readFileSync } from 'node:fs';

const [path, format] = process.argv.slice(2);
const output = readFileSync(path, 'utf8');
let found;
if (format === 'json') {
	let maxTokens = 0;
	const visit = (value) => {
		if (value === null || typeof value !== 'object') {
			return;
		}
		for (const [key, child] of Object.entries(value)) {
			if (key === 'totalTokens' && typeof child === 'number') {
				maxTokens = Math.max(maxTokens, child);
			}
			visit(child);
		}
	};
	visit(JSON.parse(output));
	found = maxTokens > 0;
} else {
	found = /\bTotal\b/u.test(output);
}
if (!found) {
	console.error(`training report found no usage: ${path}`);
	process.exit(1);
}
