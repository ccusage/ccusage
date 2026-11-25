<script lang="ts">
	import { onMount } from 'svelte';

	type Theme = 'light' | 'dark';

	const STORAGE_KEY = 'ccusage-dashboard-theme';
	let theme = $state<Theme>('light');

	function readStored(): Theme | null {
		try {
			const stored = localStorage.getItem(STORAGE_KEY);
			return stored === 'light' || stored === 'dark' ? stored : null;
		} catch {
			return null;
		}
	}

	function systemTheme(): Theme {
		return matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
	}

	onMount(() => {
		const stored = readStored();
		if (stored) document.documentElement.dataset.theme = stored;
		theme = stored ?? systemTheme();
		const media = matchMedia('(prefers-color-scheme: dark)');
		const follow = () => {
			if (!document.documentElement.hasAttribute('data-theme')) theme = systemTheme();
		};
		media.addEventListener('change', follow);
		return () => media.removeEventListener('change', follow);
	});

	function toggle() {
		theme = theme === 'dark' ? 'light' : 'dark';
		document.documentElement.dataset.theme = theme;
		try {
			localStorage.setItem(STORAGE_KEY, theme);
		} catch {
			return;
		}
	}
</script>

<button
	class="theme-toggle"
	type="button"
	aria-pressed={theme === 'dark'}
	aria-label={`Switch to ${theme === 'dark' ? 'light' : 'dark'} color theme`}
	onclick={toggle}
>
	<svg
		width="16"
		height="16"
		viewBox="0 0 24 24"
		fill="none"
		stroke="currentColor"
		stroke-width="1.5"
		aria-hidden="true"
	>
		<path d="M20.5 13A8.5 8.5 0 0 1 11 3.5 8.5 8.5 0 1 0 20.5 13Z" />
	</svg>
</button>

<style>
	.theme-toggle {
		width: 40px;
		height: 22px;
		border: 1px solid var(--border);
		border-radius: 12px;
		display: grid;
		place-items: center;
		color: var(--muted);
		background: var(--panel);
	}
</style>
