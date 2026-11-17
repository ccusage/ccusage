import { readdirSync } from 'node:fs';
import path from 'node:path';

const NIX_CASE_HACK_SUFFIX = /~nix~case~hack~[0-9]+/g;

export function restoreCatalogIdentifiers<T extends object>(
	entries: Record<string, T>,
): Record<string, T> {
	const restored: Record<string, T> = {};
	for (const [key, entry] of Object.entries(entries)) {
		const id = key.replaceAll(NIX_CASE_HACK_SUFFIX, '');
		if (Object.hasOwn(restored, id)) {
			throw new Error(`Duplicate catalog identifier: ${id}`);
		}
		restored[id] = {
			...entry,
			...('id' in entry && typeof entry.id === 'string'
				? { id: entry.id.replaceAll(NIX_CASE_HACK_SUFFIX, '') }
				: {}),
		};
	}
	return restored;
}

function escapedFileNames(directory: string): string[] {
	return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
		const entryPath = path.join(directory, entry.name);
		if (entry.isDirectory()) {
			return escapedFileNames(entryPath);
		}
		return entry.name.match(NIX_CASE_HACK_SUFFIX) == null ? [] : [entryPath];
	});
}

export function rejectEscapedFileNames(directories: readonly string[]): void {
	const escaped = directories.flatMap(escapedFileNames);
	if (escaped.length > 0) {
		throw new Error(
			`The Nix store escaped these file names, so the catalog would drop their models; regenerate on a case-sensitive store: ${escaped.join(', ')}`,
		);
	}
}
