import { redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { PageServerLoad } from './$types';

/** Where providers send people back after they sign a connection in. */
export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const denied = params.get('error');
	if (denied) {
		return {
			message: `The provider didn't sign you in: ${params.get('error_description') ?? denied}.`
		};
	}
	const state = params.get('state');
	const code = params.get('code');
	if (!state || !code) return { message: 'The provider sent you back without a sign-in.' };
	const {
		data,
		error: problem,
		response
	} = await api(event).POST('/api/v1/connections/sign-in/complete', {
		body: { state, code }
	});
	if (!data) {
		return {
			message: problemMessage(problem, `The sign-in didn't finish (${response.status}).`)
		};
	}
	redirect(303, `/integrations/${data.integrationId}?signedIn=${encodeURIComponent(data.name)}`);
};
