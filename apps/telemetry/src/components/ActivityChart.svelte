<script lang="ts">
	import { mountChart } from '@tanstack/charts';

	import {
		type ActivityDay,
		type ActivityPoint,
		createChartDefinition,
		readChartColors,
		visibleDays,
	} from '../dashboard/chart.ts';
	import { numberFormat } from '../dashboard/format.ts';

	const CHART_LABEL =
		'Daily, weekly and monthly active environments. Exact values are in the data table below.';

	let {
		days,
		now,
		unavailable,
	}: { days: readonly ActivityDay[]; now: number; unavailable: boolean } = $props();

	const periods = [30, 90] as const;
	let period = $state<(typeof periods)[number]>(30);
	let themeVersion = $state(0);
	let chartElement: HTMLDivElement;

	const visible = $derived(visibleDays(days, period, now));

	$effect(() => {
		const root = document.documentElement;
		const bump = () => themeVersion++;
		const observer = new MutationObserver(bump);
		observer.observe(root, { attributes: true, attributeFilter: ['data-theme'] });
		const media = matchMedia('(prefers-color-scheme: dark)');
		media.addEventListener('change', bump);
		return () => {
			observer.disconnect();
			media.removeEventListener('change', bump);
		};
	});

	$effect(() => {
		void themeVersion;
		const chart = mountChart<ActivityPoint, Date, number>(chartElement, {
			definition: createChartDefinition(visible, readChartColors(document.documentElement)),
			height: 300,
			initialWidth: Math.max(320, chartElement.clientWidth),
			ariaLabel: CHART_LABEL,
		});
		return () => chart.destroy();
	});
</script>

<section class="panel" aria-labelledby="activity-title">
	<div class="toolbar">
		<h2 id="activity-title">Activity over time</h2>
		<div class="periods" aria-label="Chart period">
			{#each periods as option (option)}
				<button type="button" aria-pressed={period === option} onclick={() => (period = option)}>
					{option} days
				</button>
			{/each}
		</div>
	</div>
	<div class="legend"><span>Daily</span><span>Weekly</span><span>Monthly</span></div>
	<div bind:this={chartElement} class="chart" role="img" aria-label={CHART_LABEL}></div>
	{#if unavailable}
		<p class="empty">Statistics are temporarily unavailable.</p>
	{:else if visible.rows.length === 0}
		<p class="empty">Activity will appear here after the first daily aggregate.</p>
	{/if}
	<p class="meta">
		Missing daily aggregates are gaps, not zero activity. Rolling counts are distinct IDs within
		each window; daily totals cannot be added to calculate them.
	</p>
	<details>
		<summary>View activity data as a table</summary>
		<div class="table-scroll">
			<table>
				<thead>
					<tr>
						<th scope="col">UTC day</th>
						<th scope="col">Daily</th>
						<th scope="col">Weekly</th>
						<th scope="col">Monthly</th>
					</tr>
				</thead>
				<tbody>
					{#each visible.rows.toReversed() as row (row.date)}
						<tr>
							<th scope="row">{row.date}</th>
							<td>{numberFormat.format(row.dailyActiveUsers)}</td>
							<td>{numberFormat.format(row.weeklyActiveUsers)}</td>
							<td>{numberFormat.format(row.monthlyActiveUsers)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	</details>
</section>

<style>
	.toolbar {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 16px;
		flex-wrap: wrap;
	}
	.periods {
		display: flex;
		gap: 4px;
		padding: 3px;
		background: var(--panel);
		border-radius: 8px;
	}
	.periods button {
		font-size: 12px;
		border: 0;
		border-radius: 5px;
		padding: 4px 12px;
		background: transparent;
		color: var(--muted);
	}
	.periods button[aria-pressed='true'] {
		background: var(--bg);
		color: var(--text);
		box-shadow: 0 1px 3px #00000014;
	}
	.legend {
		display: flex;
		gap: 20px;
		flex-wrap: wrap;
		font-size: 12px;
		color: var(--muted);
		padding: 20px 0;
	}
	.legend span:before {
		content: '';
		display: inline-block;
		width: 8px;
		height: 8px;
		border-radius: 50%;
		margin-right: 7px;
		background: var(--daily);
	}
	.legend span:nth-child(2):before {
		background: var(--weekly);
	}
	.legend span:nth-child(3):before {
		background: var(--monthly);
	}
	.chart {
		width: 100%;
		height: 300px;
		min-width: 0;
	}
	.empty {
		text-align: center;
		font-size: 14px;
		margin: 0;
		padding: 12px;
	}
	details {
		margin-top: 20px;
	}
	summary {
		cursor: pointer;
		font-size: 13px;
		color: var(--muted);
	}
	.table-scroll {
		overflow: auto;
		max-height: 360px;
		margin-top: 12px;
	}
	table {
		border-collapse: collapse;
		width: 100%;
		font-size: 13px;
		text-align: right;
		font-variant-numeric: tabular-nums;
	}
	th,
	td {
		padding: 10px 12px;
		border-bottom: 1px solid var(--border);
	}
	th:first-child,
	td:first-child {
		text-align: left;
	}
	@media (max-width: 700px) {
		.toolbar {
			gap: 12px;
		}
		.chart {
			height: 260px;
		}
	}
</style>
