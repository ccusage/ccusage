import { defineHandler, defineHead } from 'void';

import type { Props } from '../src/dashboard/load.ts';
import { loadDashboard } from '../src/dashboard/load.ts';

export type { Props };

export const loader = defineHandler(loadDashboard);

export const head = defineHead<Props>(() => ({
	title: 'Usage statistics | ccusage',
	htmlAttrs: { lang: 'en' },
	meta: [
		{ charset: 'utf-8' },
		{ name: 'viewport', content: 'width=device-width,initial-scale=1' },
		{
			name: 'description',
			content:
				'Public ccusage activity statistics, aggregated daily without publishing installation identifiers.',
		},
	],
}));
