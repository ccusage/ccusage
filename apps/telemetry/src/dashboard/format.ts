export const DAY_MS = 24 * 60 * 60 * 1000;

export const numberFormat = new Intl.NumberFormat('en-GB');

export const dateFormat = new Intl.DateTimeFormat('en-GB', {
	day: 'numeric',
	month: 'short',
	year: 'numeric',
	timeZone: 'UTC',
});

export function utcDay(date: string): number {
	return Date.parse(`${date}T00:00:00Z`);
}

export function trailingDates(latest: string, count: number): string[] {
	const end = utcDay(latest);
	return Array.from({ length: count }, (_, index) =>
		new Date(end - (count - 1 - index) * DAY_MS).toISOString().slice(0, 10),
	);
}
