export function restoreCatalogIdentifiers<T extends object>(
	entries: Record<string, T>,
): Record<string, T> {
	const restored: Record<string, T> = {};
	for (const [key, entry] of Object.entries(entries)) {
		const id = key.replaceAll(/~nix~case~hack~[0-9]+/g, '');
		if (Object.hasOwn(restored, id)) {
			throw new Error(`Duplicate catalog identifier: ${id}`);
		}
		restored[id] = {
			...entry,
			...('id' in entry && typeof entry.id === 'string'
				? { id: entry.id.replaceAll(/~nix~case~hack~[0-9]+/g, '') }
				: {}),
		};
	}
	return restored;
}
