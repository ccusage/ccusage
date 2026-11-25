<script lang="ts">
	let { title, value, caption }: { title: string; value: string; caption: string } = $props();
</script>

<article class="card">
	<h2>{title}</h2>
	<div class="number">{value}</div>
	<small>{caption}</small>
</article>

<style>
	.card {
		background: var(--panel);
		border: 1px solid transparent;
		border-radius: 12px;
		padding: 24px;
	}
	h2 {
		font-size: 14px;
		line-height: 22px;
		color: var(--muted);
		font-weight: 500;
		letter-spacing: 0;
	}
	.number {
		font-size: 40px;
		line-height: 1.6;
		font-weight: 600;
		letter-spacing: -0.04em;
		font-variant-numeric: tabular-nums;
	}
	small {
		font-size: 11px;
		color: var(--muted);
	}
	@media (max-width: 700px) {
		.card {
			padding: 16px 20px;
		}
		.number {
			font-size: 36px;
		}
	}
</style>
