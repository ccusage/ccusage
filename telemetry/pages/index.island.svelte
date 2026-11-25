<script lang="ts">
	import faviconUrl from '../../docs/public/favicon.svg?url';
	import '../src/global.css';
	import ActivityChart from '../src/components/ActivityChart.svelte' with { island: 'load' };
	import ShareHistory from '../src/components/ShareHistory.svelte' with { island: 'visible' };
	import ShareSummary from '../src/components/ShareSummary.svelte';
	import SiteNav from '../src/components/SiteNav.svelte';
	import StatCard from '../src/components/StatCard.svelte';
	import ThemeToggle from '../src/components/ThemeToggle.svelte' with { island: 'load' };
	import { DAY_MS, numberFormat, trailingDates, utcDay } from '../src/dashboard/format.ts';
	import { shareView } from '../src/dashboard/share.ts';
	import type { Props } from './index.server.ts';

	let { stats, unavailable, now }: Props = $props();

	const days = $derived(stats?.days ?? []);
	const latest = $derived(days.at(-1));
	const sampled = $derived(days.some((day) => day.samplingObserved === true));
	const inconsistent = $derived(
		days.some(
			(day) =>
				day.dailyActiveUsers > day.weeklyActiveUsers ||
				day.weeklyActiveUsers > day.monthlyActiveUsers,
		),
	);
	const status = $derived(
		unavailable
			? 'Statistics could not be loaded. Please try again later, or check the aggregate JSON endpoint.'
			: latest === undefined
				? 'No daily aggregates yet. Statistics will appear after the first successful daily rollup.'
				: now - utcDay(latest.date) >= 2 * DAY_MS
					? 'The latest aggregate is older than expected. Recent activity is not yet available.'
					: null,
	);
	const quality = $derived(
		[
			sampled ? 'Some counts were read from sampled data and may be incomplete.' : '',
			inconsistent
				? 'Some daily, weekly and monthly counts are inconsistent. Treat these figures as incomplete; inspect the aggregate JSON for the affected dates.'
				: '',
		]
			.filter(Boolean)
			.join(' '),
	);
	const activity = $derived(
		days.map(({ date, dailyActiveUsers, weeklyActiveUsers, monthlyActiveUsers }) => ({
			date,
			dailyActiveUsers,
			weeklyActiveUsers,
			monthlyActiveUsers,
		})),
	);
	const count = (value: number | undefined) => (value === undefined ? '—' : numberFormat.format(value));
	const emptyBreakdown = $derived(latest ? 'No recorded activity.' : 'No recorded activity yet.');
	const shareDates = $derived(latest ? trailingDates(latest.date, 30) : []);
	const platforms = $derived(shareView(days, 'platform', shareDates));
	const versions = $derived(shareView(days, 'version', shareDates));
</script>

<svelte:head>
	<link rel="icon" type="image/svg+xml" href={faviconUrl} />
</svelte:head>

<a class="skip-link" href="#main">Skip to content</a>
<SiteNav>
	{#snippet actions()}
		<ThemeToggle />
	{/snippet}
</SiteNav>
<main id="main">
	<header>
		<h1>Usage statistics</h1>
		<p class="intro">
			Daily activity from participating ccusage installations, shared openly with the community.
		</p>
	</header>
	{#if status}
		<div class="notice" role="status">{status}</div>
	{/if}
	{#if quality}
		<div class="notice" role="status">{quality}</div>
	{/if}
	<section aria-label="Public activity statistics">
		<p class="meta">
			{latest
				? `Latest activity: ${latest.date} UTC · Aggregated ${stats?.generatedAt}`
				: 'No daily aggregate has been published yet.'}
		</p>
		<div class="cards">
			<StatCard
				title="Daily active environments"
				value={count(latest?.dailyActiveUsers)}
				caption="Distinct installation IDs · 1 UTC day"
			/>
			<StatCard
				title="Weekly active environments"
				value={count(latest?.weeklyActiveUsers)}
				caption="Distinct installation IDs · 7 UTC days"
			/>
			<StatCard
				title="Monthly active environments"
				value={count(latest?.monthlyActiveUsers)}
				caption="Distinct installation IDs · 30 UTC days"
			/>
		</div>
		<ActivityChart days={activity} {now} {unavailable} />
		<div class="split">
			<section class="panel" aria-labelledby="platforms-title">
				<h2 id="platforms-title">Platforms</h2>
				<p class="meta">
					Latest available day · up to 20 groups · shares of daily active environments
				</p>
				<ShareSummary view={platforms} emptyLabel={emptyBreakdown} />
				<ShareHistory
					view={platforms}
					label="Daily share of active environments by platform over the last 30 UTC days. Exact values are in the table below."
				/>
			</section>
			<section class="panel" aria-labelledby="versions-title">
				<h2 id="versions-title">Versions</h2>
				<p class="meta">
					Latest available day · up to 20 versions · shares of daily active environments
				</p>
				<ShareSummary view={versions} emptyLabel={emptyBreakdown} />
				<ShareHistory
					view={versions}
					label="Daily share of active environments by version over the last 30 UTC days. Exact values are in the table below."
				/>
			</section>
		</div>
	</section>
	<section class="method" aria-labelledby="method-title">
		<h2 id="method-title">What these numbers mean</h2>
		<p>
			An active environment is a distinct random ID observed during a UTC window, not a verified
			person. Multiple devices, reset IDs, opt-outs, blocked requests and failed sends affect the
			counts. These are adoption indicators, not an exact population or a guaranteed lower bound.
		</p>
		<p>
			Analytics Engine can sample data, especially over longer windows. Sampling can omit activity,
			and its interval does not provide an error bound. The dashboard flags observed sampling and
			inconsistent daily, weekly or monthly counts without changing the figures.
		</p>
		<p>
			The CLI sends at most one ping per UTC day with an installation ID, version, OS, architecture
			and command name. Nothing from usage logs, prompts or costs is sent. This page publishes
			aggregate counts only, never installation IDs. Command breakdowns are omitted because the
			first command of the day does not represent all command usage.
		</p>
		<p>
			Aggregates are generated daily after 00:15 UTC. This page and the public API are cached at
			the edge until the next aggregate and in browsers for up to five minutes. No page analytics or
			third-party scripts are used.
		</p>
		<p>
			To opt out, run <code>ccusage telemetry disable</code> or set <code>DO_NOT_TRACK=1</code>.
			<a href="https://ccusage.com/guide/telemetry">Read the collection and storage policy</a>.
		</p>
	</section>
	<footer>
		<a href="/v1/stats">Download aggregate JSON</a>
		<a href="https://github.com/ccusage/ccusage/tree/main/telemetry">Inspect the source</a>
		<a href="https://github.com/sponsors/ryoppippi">Support ccusage</a>
	</footer>
</main>

<style>
	header {
		max-width: 760px;
		margin-bottom: 32px;
	}
	.intro {
		font-size: 16px;
		line-height: 28px;
	}
	.notice {
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 12px 16px;
		margin: 24px 0;
		background: var(--panel);
		font-size: 14px;
		color: var(--muted);
	}
	.cards {
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		gap: 16px;
		margin: 24px 0;
	}
	.split {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 16px;
		margin: 16px 0;
	}
	.method {
		margin-top: 40px;
		border-top: 1px solid var(--border);
		padding-top: 24px;
	}
	.method h2 {
		margin-bottom: 16px;
	}
	.method p {
		font-size: 14px;
		line-height: 26px;
		max-width: 860px;
	}
	footer {
		border-top: 1px solid var(--border);
		padding-top: 24px;
		margin-top: 32px;
		font-size: 13px;
		color: var(--muted);
		display: flex;
		gap: 24px;
		flex-wrap: wrap;
	}
	@media (max-width: 700px) {
		.cards {
			grid-template-columns: 1fr;
			gap: 12px;
		}
		.split {
			grid-template-columns: 1fr;
		}
	}
</style>
