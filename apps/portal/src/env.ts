import { defineEnvVars } from '@sveltejs/kit/env';

export const variables = defineEnvVars({
	HUXLEY_API_URL: {
		description:
			'Internal URL of the Huxley API, used by server-side code only. Browsers never call it directly.',
		schema: (value) => value || 'http://localhost:8080'
	}
});
