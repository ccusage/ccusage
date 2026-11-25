<script lang="ts">
	import { mountChart } from '@tanstack/charts';

	import {
		createShareDefinition,
		readChartColors,
		readSeriesColors,
	} from '../dashboard/chart.ts';
	import { numberFormat, utcDay, dateFormat } from '../dashboard/format.ts';
	import { seriesColorVariable, type ShareView } from '../dashboard/share.ts';

	let { view, label }: { view: ShareView; label: string } = $props();

	let themeVersion = $state(0);
	let chartElement: HTMLDivElement;
	const percent = new Intl.NumberFormat('en-GB', { style: 'percent', maximumFractionDigits: 1 });
	const daysWithData = $derived([...new Set(view.rows.map((row) => row.date))].toReversed());

	$effect(() => {
		const bump = () => themeVersion++;
		const observer = new MutationObserver(bump);
		observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
		const media = matchMedia('(prefers-color-scheme: dark)');
		media.addEventListener('change', bump);
		return () => {
			observer.disconnect();
			media.removeEventListener('change', bump);
		};
	});

	$effect(() => {
		void themeVersion;
		if (view.rows.length === 0) return;
		const root = document.documentElement;
		const chart = mountChart(chartElement, {
			definition: createShareDefinition(
				view,
				readChartColors(root),
				readSeriesColors(root, view.keys),
			),
			height: 200,
			initialWidth: Math.max(280, chartElement.clientWidth),
			ariaLabel: label,
		});
		return () => chart.destroy();
	});
</script>

{#if view.rows.length > 0}
	<h3>Share by UTC day</h3>
	<ul class="legend">
		{#each view.keys as key (key)}
			<li>
				<span class="swatch" style:background={`var(${seriesColorVariable(view.keys, key)})`}></span>
				{key}
			</li>
		{/each}
	</ul>
	<div bind:this={chartElement} class="chart" role="img" aria-label={label}></div>
	<details>
		<summary>View daily shares as a table</summary>
		<div class="table-scroll">
			<table>
				<thead>
					<tr>
						<th scope="col">UTC day</th>
						{#each view.keys as key (key)}
							<th scope="col">{key}</th>
						{/each}
					</tr>
				</thead>
				<tbody>
					{#each daysWithData as date (date)}
						<tr>
							<th scope="row">{dateFormat.format(utcDay(date))}</th>
							{#each view.rows.filter((row) => row.date === date) as row (row.key)}
								<td>{numberFormat.format(row.users)} ({percent.format(row.share)})</td>
							{/each}
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	</details>
{/if}

<style>
	h3 {
		margin: 24px 0 8px;
		font-size: 14px;
		font-weight: 500;
		color: var(--muted);
	}
	.legend {
		display: flex;
		flex-wrap: wrap;
		gap: 6px 16px;
		padding: 0;
		margin: 0 0 8px;
		list-style: none;
		font-size: 12px;
		color: var(--muted);
	}
	.legend li {
		display: inline-flex;
		align-items: center;
		gap: 6px;
	}
	.swatch {
		width: 8px;
		height: 8px;
		border-radius: 2px;
	}
	.chart {
		width: 100%;
		height: 200px;
		min-width: 0;
	}
	details {
		margin-top: 12px;
	}
	summary {
		cursor: pointer;
		font-size: 13px;
		color: var(--muted);
	}
	.table-scroll {
		overflow: auto;
		max-height: 320px;
		margin-top: 12px;
	}
	table {
		border-collapse: collapse;
		width: 100%;
		font-size: 12px;
		text-align: right;
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}
	th,
	td {
		padding: 8px 10px;
		border-bottom: 1px solid var(--border);
	}
	th:first-child {
		text-align: left;
	}
</style>
