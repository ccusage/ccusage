<script lang="ts">
	import { numberFormat } from '../dashboard/format.ts';
	import { OTHER_KEY, seriesColorVariable, type ShareView } from '../dashboard/share.ts';

	let { view, emptyLabel }: { view: ShareView; emptyLabel: string } = $props();

	const percent = new Intl.NumberFormat('en-GB', { style: 'percent', maximumFractionDigits: 1 });
	const latest = $derived(view.latest);
	const segments = $derived(
		latest === null
			? []
			: view.rows
					.filter((row) => row.date === latest.date && row.users > 0)
					.map((row) => ({ ...row, color: seriesColorVariable(view.keys, row.key) })),
	);
	const share = (users: number) =>
		latest === null || latest.total === 0 ? '' : percent.format(users / latest.total);
	const swatch = (label: string) =>
		seriesColorVariable(view.keys, view.keys.includes(label) ? label : OTHER_KEY);
</script>

{#if latest && segments.length > 0}
	<div class="bar" aria-hidden="true">
		{#each segments as segment (segment.key)}
			<span style:flex-grow={segment.users} style:background={`var(${segment.color})`}></span>
		{/each}
	</div>
{/if}
<ul>
	{#each latest?.groups ?? [] as group (group.label)}
		<li>
			<span class="swatch" style:background={`var(${swatch(group.label)})`} aria-hidden="true"></span>
			<span class="label">{group.label}</span>
			<span class="users">{numberFormat.format(group.users)}</span>
			<span class="share">{share(group.users)}</span>
		</li>
	{:else}
		<li class="empty">{emptyLabel}</li>
	{/each}
</ul>

<style>
	.bar {
		display: flex;
		gap: 2px;
		height: 10px;
		margin: 16px 0 8px;
	}
	.bar span {
		flex-basis: 0;
		min-width: 2px;
		border-radius: 2px;
	}
	ul {
		padding: 0;
		list-style: none;
		margin: 0;
	}
	li {
		display: grid;
		grid-template-columns: 10px 1fr auto 4.5em;
		align-items: center;
		gap: 10px;
		padding: 8px 0;
		border-bottom: 1px solid var(--border);
		font-size: 14px;
		font-variant-numeric: tabular-nums;
	}
	li:last-child {
		border-bottom: 0;
	}
	.swatch {
		width: 10px;
		height: 10px;
		border-radius: 2px;
	}
	.label {
		overflow-wrap: anywhere;
	}
	.share {
		text-align: right;
		color: var(--muted);
	}
	.empty {
		display: block;
		color: var(--muted);
	}
</style>
