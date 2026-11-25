<script lang="ts">
	import type { Snippet } from 'svelte';

	let { actions }: { actions: Snippet } = $props();
</script>

<nav class="site-nav" aria-label="Main navigation">
	<div class="nav-inner">
		<a class="brand" href="https://ccusage.com"><span class="brand-logo" aria-hidden="true"></span>ccusage</a>
		<div class="nav-links">
			<a href="https://ccusage.com/guide/">Guide</a>
			<a class="blog" href="https://ccusage.com/blog/">Blog</a>
			<a href="/" aria-current="page">Statistics</a>
			<span class="divider"></span>
			{@render actions()}
			<a class="github" href="https://github.com/ccusage/ccusage" aria-label="ccusage on GitHub">
				<svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
					<path
						d="M12 .75a11.25 11.25 0 0 0-3.56 21.92c.56.1.77-.24.77-.54v-2.1c-3.13.68-3.79-1.33-3.79-1.33-.51-1.3-1.25-1.64-1.25-1.64-1.02-.7.08-.68.08-.68 1.13.08 1.72 1.16 1.72 1.16 1 .1.98 1.94 3.28 1.35.1-.73.39-1.23.71-1.51-2.5-.29-5.13-1.25-5.13-5.56 0-1.23.44-2.23 1.16-3.02-.12-.29-.5-1.43.11-2.98 0 0 .95-.31 3.09 1.15a10.76 10.76 0 0 1 5.62 0c2.14-1.46 3.08-1.15 3.08-1.15.62 1.55.23 2.69.12 2.98.72.79 1.16 1.79 1.16 3.02 0 4.32-2.63 5.26-5.14 5.54.4.35.76 1.04.76 2.1v3.14c0 .3.2.65.78.54A11.25 11.25 0 0 0 12 .75Z"
					/>
				</svg>
			</a>
		</div>
	</div>
</nav>

<style>
	.site-nav {
		height: 64px;
		border-bottom: 1px solid var(--border);
	}
	.nav-inner {
		height: 100%;
		max-width: 1440px;
		padding: 0 32px;
		margin: auto;
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 24px;
	}
	.brand {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		font-size: 16px;
		font-weight: 600;
		color: var(--text);
		text-decoration: none;
	}
	.brand-logo {
		display: inline-block;
		width: 24px;
		height: 24px;
		flex-shrink: 0;
		border-radius: 6px;
		background: url('../../../docs/public/logo.svg') center / contain no-repeat;
	}
	.nav-links {
		display: flex;
		align-items: center;
		gap: 24px;
		font-size: 14px;
		font-weight: 500;
	}
	.nav-links a {
		color: var(--text);
		text-decoration: none;
	}
	.nav-links a:hover,
	.nav-links a[aria-current='page'] {
		color: var(--accent);
	}
	.divider {
		height: 24px;
		width: 1px;
		background: var(--border);
	}
	.github {
		display: flex;
		align-items: center;
	}
	@media (max-width: 700px) {
		.nav-inner {
			padding: 0 20px;
		}
		.nav-links {
			gap: 16px;
		}
		.blog,
		.divider {
			display: none;
		}
	}
</style>
