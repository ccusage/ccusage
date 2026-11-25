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
