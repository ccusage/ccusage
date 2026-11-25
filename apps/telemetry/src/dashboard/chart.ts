import { defineChart, lineY } from '@tanstack/charts';
import { scaleLinear } from '@tanstack/charts/scales/linear';
import { scaleUtc } from '@tanstack/charts/scales/time';
import { tooltip } from '@tanstack/charts/tooltip';

import { DAY_MS, dateFormat, numberFormat, utcDay } from './format.ts';

export type ActivityDay = {
	date: string;
	dailyActiveUsers: number;
	weeklyActiveUsers: number;
	monthlyActiveUsers: number;
};
export type ActivityPoint = { date: Date; value: number; label: string };
export type ChartColors = {
	daily: string;
	weekly: string;
	monthly: string;
	border: string;
	muted: string;
	background: string;
};

const series = [
	{ key: 'dailyActiveUsers', label: 'Daily', color: 'daily' },
	{ key: 'weeklyActiveUsers', label: 'Weekly', color: 'weekly' },
	{ key: 'monthlyActiveUsers', label: 'Monthly', color: 'monthly' },
] as const;

export function readChartColors(root: HTMLElement): ChartColors {
	const styles = getComputedStyle(root);
	const color = (name: string) => styles.getPropertyValue(name).trim();
	return {
		daily: color('--daily'),
		weekly: color('--weekly'),
		monthly: color('--monthly'),
		border: color('--border'),
		muted: color('--muted'),
		background: color('--bg'),
	};
}

export function visibleDays(
	days: readonly ActivityDay[],
	windowDays: number,
	now: number,
): { start: Date; end: Date; rows: ActivityDay[] } {
	const latest = days.at(-1);
	const end = new Date(latest ? utcDay(latest.date) : Math.floor(now / DAY_MS) * DAY_MS - DAY_MS);
	const start = new Date(end.getTime() - (windowDays - 1) * DAY_MS);
	const rows = days.filter((row) => {
		const day = utcDay(row.date);
		return day >= start.getTime() && day <= end.getTime();
	});
	return { start, end, rows };
}

export function createChartDefinition(
	visible: ReturnType<typeof visibleDays>,
	colors: ChartColors,
) {
	const { start, end, rows } = visible;
	const maximum = Math.max(1, ...rows.flatMap((row) => series.map(({ key }) => row[key])));
	const marks = series.flatMap(({ key, label, color }) => {
		const runs: ActivityPoint[][] = [];
		let run: ActivityPoint[] = [];
		let previous = -DAY_MS;
		for (const row of rows) {
			const timestamp = utcDay(row.date);
			if (timestamp !== previous + DAY_MS && run.length > 0) {
				runs.push(run);
				run = [];
			}
			run.push({ date: new Date(timestamp), value: row[key], label });
			previous = timestamp;
		}
		if (run.length > 0) runs.push(run);
		return (rows.length > 0 ? runs : [[]]).map((points, index) =>
			lineY(points, {
				x: 'date',
				y: 'value',
				id: `${label.toLowerCase()}-${index}`,
				stroke: colors[color],
				strokeWidth: 2.5,
				points: true,
			}),
		);
	});

	return defineChart({
		marks,
		theme: {
			foreground: colors.muted,
			muted: colors.muted,
			grid: colors.border,
			background: colors.background,
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
	});
}
