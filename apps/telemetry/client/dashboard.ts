import faviconUrl from '../../../docs/public/favicon.svg?url';
import { defineChart, lineY, mountChart } from '@tanstack/charts';
import { scaleLinear } from '@tanstack/charts/scales/linear';
import { scaleUtc } from '@tanstack/charts/scales/time';
import { tooltip } from '@tanstack/charts/tooltip';

const DAY_MS = 24 * 60 * 60 * 1000;
const THEME_STORAGE_KEY = 'ccusage-dashboard-theme';
const numberFormat = new Intl.NumberFormat('en-GB');
const dateFormat = new Intl.DateTimeFormat('en-GB', {
	day: 'numeric',
	month: 'short',
	year: 'numeric',
	timeZone: 'UTC',
});

type Breakdown = { label: string; users: number };
type DailyStat = {
	date: string;
	dailyActiveUsers: number;
	weeklyActiveUsers: number;
	monthlyActiveUsers: number;
	byPlatform: { os: string; arch: string; users: number }[];
	byVersion: { version: string; users: number }[];
};
type PublicStats = { generatedAt: string | null; days: DailyStat[] };
type ActivityPoint = { date: Date; value: number; label: string };
type Series = {
	key: 'dailyActiveUsers' | 'weeklyActiveUsers' | 'monthlyActiveUsers';
	label: string;
	color: string;
};

const series: readonly Series[] = [
	{ key: 'dailyActiveUsers', label: 'Daily', color: '--daily' },
	{ key: 'weeklyActiveUsers', label: 'Weekly', color: '--weekly' },
	{ key: 'monthlyActiveUsers', label: 'Monthly', color: '--monthly' },
];

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isCount(value: unknown): value is number {
	return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
}

function parseStats(value: unknown): PublicStats | null {
	if (
		!isRecord(value) ||
		(value.generatedAt !== null && typeof value.generatedAt !== 'string') ||
		!Array.isArray(value.days)
	) {
		return null;
	}
	if (value.days.length > 0 && value.generatedAt === null) return null;
	const days: DailyStat[] = [];
	let previousDate = '';
	for (const entry of value.days) {
		if (
			!isRecord(entry) ||
			typeof entry.date !== 'string' ||
			!/^\d{4}-\d{2}-\d{2}$/.test(entry.date) ||
			entry.date <= previousDate ||
			Number.isNaN(Date.parse(`${entry.date}T00:00:00Z`)) ||
			!isCount(entry.dailyActiveUsers) ||
			!isCount(entry.weeklyActiveUsers) ||
			!isCount(entry.monthlyActiveUsers)
		) {
			return null;
		}
		if (!Array.isArray(entry.byPlatform) || !Array.isArray(entry.byVersion)) return null;
		const byPlatform: DailyStat['byPlatform'] = [];
		for (const platform of entry.byPlatform) {
			if (
				!isRecord(platform) ||
				typeof platform.os !== 'string' ||
				typeof platform.arch !== 'string' ||
				!isCount(platform.users)
			) {
				return null;
			}
			byPlatform.push({ os: platform.os, arch: platform.arch, users: platform.users });
		}
		const byVersion: DailyStat['byVersion'] = [];
		for (const version of entry.byVersion) {
			if (!isRecord(version) || typeof version.version !== 'string' || !isCount(version.users)) {
				return null;
			}
			byVersion.push({ version: version.version, users: version.users });
		}
		days.push({
			date: entry.date,
			dailyActiveUsers: entry.dailyActiveUsers,
			weeklyActiveUsers: entry.weeklyActiveUsers,
			monthlyActiveUsers: entry.monthlyActiveUsers,
			byPlatform,
			byVersion,
		});
		previousDate = entry.date;
	}
	return { generatedAt: value.generatedAt, days };
}

function element<T extends HTMLElement>(id: string): T {
	const found = document.getElementById(id);
	if (!found) throw new Error(`Missing dashboard element: ${id}`);
	return found as T;
}

function makeNode<K extends keyof HTMLElementTagNameMap>(
	tag: K,
	text?: string,
): HTMLElementTagNameMap[K] {
	const result = document.createElement(tag);
	if (text !== undefined) result.textContent = text;
	return result;
}

function utcDay(date: string): number {
	return Date.parse(`${date}T00:00:00Z`);
}

function readThemePreference(): 'light' | 'dark' | null {
	try {
		const stored = localStorage.getItem(THEME_STORAGE_KEY);
		return stored === 'light' || stored === 'dark' ? stored : null;
	} catch {
		return null;
	}
}

function storeThemePreference(theme: 'light' | 'dark'): void {
	try {
		localStorage.setItem(THEME_STORAGE_KEY, theme);
	} catch {
		return;
	}
}

function readChartColors(): {
	daily: string;
	weekly: string;
	monthly: string;
	border: string;
	muted: string;
} {
	const styles = getComputedStyle(document.documentElement);
	const color = (name: string, fallback: string) =>
		styles.getPropertyValue(name).trim() || fallback;
	return {
		daily: color('--daily', '#3451b2'),
		weekly: color('--weekly', '#18794e'),
		monthly: color('--monthly', '#a65d0b'),
		border: color('--border', '#e2e2e3'),
		muted: color('--muted', '#67676c'),
	};
}

function emptyPlotWindow(days: number): { start: Date; end: Date } {
	const end = new Date(Math.floor(Date.now() / DAY_MS) * DAY_MS - DAY_MS);
	return { start: new Date(end.getTime() - (days - 1) * DAY_MS), end };
}

function createChartDefinition(
	rows: readonly DailyStat[],
	windowDays: number,
	colors: ReturnType<typeof readChartColors>,
) {
	const latest = rows.at(-1);
	const end = latest ? new Date(utcDay(latest.date)) : emptyPlotWindow(windowDays).end;
	const start = new Date(end.getTime() - (windowDays - 1) * DAY_MS);
	const visibleRows = rows.filter((row) => {
		const day = utcDay(row.date);
		return day >= start.getTime() && day <= end.getTime();
	});
	const hasActivity = visibleRows.length > 0;
	const allValues = visibleRows.flatMap((row) => series.map(({ key }) => row[key]));
	const maximum = Math.max(1, ...allValues);
	const chartSeries = series.map((item) => ({
		...item,
		stroke:
			item.color === '--daily'
				? colors.daily
				: item.color === '--weekly'
					? colors.weekly
					: colors.monthly,
	}));
	const marks = chartSeries.flatMap(({ key, label, stroke }) => {
		const runs: ActivityPoint[][] = [];
		let run: ActivityPoint[] = [];
		let previous = -DAY_MS;
		for (const row of visibleRows) {
			const timestamp = utcDay(row.date);
			if (timestamp !== previous + DAY_MS && run.length > 0) {
				runs.push(run);
				run = [];
			}
			run.push({ date: new Date(timestamp), value: row[key], label });
			previous = timestamp;
		}
		if (run.length > 0) runs.push(run);
		return (hasActivity ? runs : [[]]).map((points, index) =>
			lineY(points, {
				x: 'date',
				y: 'value',
				id: `${label.toLowerCase()}-${index}`,
				stroke,
				strokeWidth: 2.5,
				points: true,
			}),
		);
	});

	return {
		definition: defineChart({
			marks,
			theme: {
				foreground: colors.muted,
				muted: colors.muted,
				grid: colors.border,
				background: getComputedStyle(document.documentElement).getPropertyValue('--bg').trim(),
				palette: [colors.daily, colors.weekly, colors.monthly],
			},
			scales: {
				x: {
					scale: scaleUtc().domain([start, end]),
					grid: { stroke: colors.border, strokeOpacity: 0.65 },
					axis: {
						ticks: { count: 5, format: (value) => dateFormat.format(value) },
						tickLabels: { fontSize: 12, thin: { priority: 'ends' } },
					},
				},
				y: {
					scale: scaleLinear().domain([0, maximum]),
					grid: { stroke: colors.border, strokeOpacity: 0.65 },
					axis: {
						ticks: { format: (value) => numberFormat.format(value) },
						tickLabels: { fontSize: 12 },
					},
				},
			},
			tooltip: {
				use: tooltip,
				format: (point) =>
					`${dateFormat.format(point.xValue)} · ${point.datum.label}: ${numberFormat.format(point.yValue)}`,
			},
		}),
		rows: visibleRows,
		hasActivity,
	};
}

function renderBreakdown(
	list: HTMLUListElement,
	rows: readonly Breakdown[],
	emptyLabel: string,
): void {
	list.replaceChildren();
	if (rows.length === 0) {
		list.append(makeNode('li', emptyLabel));
		return;
	}
	const maximum = Math.max(1, ...rows.map((row) => row.users));
	for (const row of rows) {
		const item = makeNode('li');
		const heading = makeNode('div');
		heading.className = 'row';
		heading.append(makeNode('span', row.label), makeNode('span', numberFormat.format(row.users)));
		const track = makeNode('div');
		track.className = 'track';
		track.setAttribute('aria-hidden', 'true');
		const fill = makeNode('div');
		fill.className = 'fill';
		fill.style.width = `${(row.users / maximum) * 100}%`;
		track.append(fill);
		item.append(heading, track);
		list.append(item);
	}
}

function renderTable(tbody: HTMLTableSectionElement, rows: readonly DailyStat[]): void {
	tbody.replaceChildren();
	for (const row of [...rows].reverse()) {
		const tr = makeNode('tr');
		const day = makeNode('th', row.date);
		day.scope = 'row';
		tr.append(
			day,
			makeNode('td', numberFormat.format(row.dailyActiveUsers)),
			makeNode('td', numberFormat.format(row.weeklyActiveUsers)),
			makeNode('td', numberFormat.format(row.monthlyActiveUsers)),
		);
		tbody.append(tr);
	}
}

async function startDashboard(): Promise<void> {
	element<HTMLLinkElement>('site-icon').href = faviconUrl;
	let chartHost: ReturnType<typeof mountChart<ActivityPoint, Date, number>> | undefined;
	let stats: PublicStats = { generatedAt: '', days: [] };
	let period = 30;
	const status = element<HTMLDivElement>('status');
	const chartElement = element<HTMLDivElement>('chart');
	const chartEmpty = element<HTMLParagraphElement>('chart-empty');
	const tableBody = element<HTMLTableSectionElement>('activity-rows');
	const periodButtons = [...document.querySelectorAll<HTMLButtonElement>('[data-days]')];
	const themeToggle = element<HTMLButtonElement>('theme-toggle');
	const storedTheme = readThemePreference();
	if (storedTheme) document.documentElement.dataset.theme = storedTheme;
	const systemTheme = matchMedia('(prefers-color-scheme: dark)');
	const initialTheme = storedTheme ?? (systemTheme.matches ? 'dark' : 'light');
	themeToggle.setAttribute('aria-pressed', String(initialTheme === 'dark'));
	themeToggle.setAttribute(
		'aria-label',
		`Switch to ${initialTheme === 'dark' ? 'light' : 'dark'} color theme`,
	);

	function showEmptyChart(message: string): void {
		chartEmpty.hidden = false;
		chartEmpty.textContent = message;
	}

	function refreshChart(): void {
		const colors = readChartColors();
		const chart = createChartDefinition(stats.days, period, colors);
		chartHost?.destroy();
		chartHost = mountChart(chartElement, {
			definition: chart.definition,
			height: 300,
			initialWidth: Math.max(320, chartElement.clientWidth),
			ariaLabel:
				'Daily, weekly and monthly active environments. Exact values are in the data table below.',
		});
		showEmptyChart(
			chart.hasActivity
				? 'Missing daily aggregates are gaps, not zero activity.'
				: 'Activity will appear here after the first daily aggregate.',
		);
		chartEmpty.hidden = chart.hasActivity;
		renderTable(tableBody, chart.rows);
	}

	function setTheme(theme: 'light' | 'dark', persist: boolean): void {
		document.documentElement.dataset.theme = theme;
		themeToggle.setAttribute('aria-pressed', String(theme === 'dark'));
		themeToggle.setAttribute(
			'aria-label',
			`Switch to ${theme === 'dark' ? 'light' : 'dark'} color theme`,
		);
		if (persist) storeThemePreference(theme);
		refreshChart();
	}

	themeToggle.addEventListener('click', () => {
		const current =
			document.documentElement.dataset.theme === 'dark' ||
			(document.documentElement.dataset.theme !== 'light' &&
				matchMedia('(prefers-color-scheme: dark)').matches)
				? 'dark'
				: 'light';
		setTheme(current === 'dark' ? 'light' : 'dark', true);
	});
	for (const button of periodButtons) {
		button.addEventListener('click', () => {
			const nextPeriod = Number(button.dataset.days);
			if (nextPeriod !== 30 && nextPeriod !== 90) return;
			period = nextPeriod;
			for (const other of periodButtons) {
				other.setAttribute('aria-pressed', String(other === button));
			}
			refreshChart();
		});
	}
	systemTheme.addEventListener('change', () => {
		if (!document.documentElement.hasAttribute('data-theme')) refreshChart();
	});
	refreshChart();

	try {
		const response = await fetch('/v1/stats');
		if (!response.ok) throw new Error('Statistics unavailable');
		const parsed = parseStats(await response.json());
		if (!parsed) throw new Error('Invalid statistics response');
		stats = parsed;
		const latest = stats.days.at(-1);
		if (!latest) {
			status.textContent =
				'No daily aggregates yet. Statistics will appear after the first successful daily rollup.';
			element<HTMLParagraphElement>('updated').textContent =
				'No daily aggregate has been published yet.';
			element<HTMLElement>('daily').textContent = '—';
			element<HTMLElement>('weekly').textContent = '—';
			element<HTMLElement>('monthly').textContent = '—';
			renderBreakdown(element<HTMLUListElement>('platforms'), [], 'No recorded activity yet.');
			renderBreakdown(element<HTMLUListElement>('versions'), [], 'No recorded activity yet.');
			renderTable(tableBody, []);
			refreshChart();
			return;
		}

		element<HTMLElement>('daily').textContent = numberFormat.format(latest.dailyActiveUsers);
		element<HTMLElement>('weekly').textContent = numberFormat.format(latest.weeklyActiveUsers);
		element<HTMLElement>('monthly').textContent = numberFormat.format(latest.monthlyActiveUsers);
		element<HTMLParagraphElement>('updated').textContent =
			`Latest activity: ${latest.date} UTC · Aggregated ${stats.generatedAt}`;
		renderBreakdown(
			element<HTMLUListElement>('platforms'),
			latest.byPlatform.map(({ os, arch, users }) => ({ label: `${os} / ${arch}`, users })),
			'No recorded activity.',
		);
		renderBreakdown(
			element<HTMLUListElement>('versions'),
			latest.byVersion.map(({ version, users }) => ({ label: version, users })),
			'No recorded activity.',
		);
		const age = Date.now() - utcDay(latest.date);
		status.hidden = age < 2 * DAY_MS;
		if (!status.hidden) {
			status.textContent =
				'The latest aggregate is older than expected. Recent activity is not yet available.';
		}
		refreshChart();
	} catch {
		status.hidden = false;
		status.textContent =
			'Statistics could not be loaded. Please try again later, or check the aggregate JSON endpoint.';
		refreshChart();
		showEmptyChart('Statistics are temporarily unavailable.');
	}
}

void startDashboard();
