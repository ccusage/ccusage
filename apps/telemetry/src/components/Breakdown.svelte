<script lang="ts">
	import { numberFormat } from '../dashboard/format.ts';

	type Row = { label: string; users: number };

	let {
		title,
		caption,
		rows,
		emptyLabel,
	}: { title: string; caption: string; rows: readonly Row[]; emptyLabel: string } = $props();

	const maximum = $derived(Math.max(1, ...rows.map((row) => row.users)));
</script>

<section class="panel">
	<h2>{title}</h2>
	<p class="meta">{caption}</p>
	<ul>
		{#each rows as row (row.label)}
			<li>
				<div class="row">
					<span>{row.label}</span><span>{numberFormat.format(row.users)}</span>
				</div>
				<div class="track" aria-hidden="true">
					<div class="fill" style:width={`${(row.users / maximum) * 100}%`}></div>
				</div>
			</li>
		{:else}
			<li>{emptyLabel}</li>
		{/each}
	</ul>
</section>

<style>
	ul {
		padding: 8px 0 0;
		list-style: none;
		margin: 0;
	}
	li {
		margin: 16px 0;
	}
	.row {
		display: flex;
		justify-content: space-between;
		gap: 14px;
		font-size: 14px;
		overflow-wrap: anywhere;
	}
	.track {
		height: 5px;
		border-radius: 4px;
		background: var(--panel);
		margin-top: 8px;
		overflow: hidden;
	}
	.fill {
		height: 100%;
		background: var(--accent);
		border-radius: 4px;
	}
</style>
