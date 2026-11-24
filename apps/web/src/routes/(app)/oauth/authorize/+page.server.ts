import { error, fail, redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

/**
 * The OAuth authorization endpoint for MCP clients (ADR-032): the person
 * approves or declines an app. The API checks the request and builds the URL
 * to return to; this page only shows the question and passes the answer on.
 */

const PARAMS = [
	['response_type', 'responseType'],
	['client_id', 'clientId'],
	['redirect_uri', 'redirectUri'],
	['code_challenge', 'codeChallenge'],
	['code_challenge_method', 'codeChallengeMethod'],
	['scope', 'scope'],
	['state', 'state'],
	['resource', 'resource']
] as const;

type Params = Partial<Record<(typeof PARAMS)[number][1], string>>;

function paramsFrom(get: (name: string) => string | null): Params {
	const params: Params = {};
	for (const [name, key] of PARAMS) {
		const value = get(name);
		if (value !== null && value !== '') params[key] = value;
	}
	return params;
}

export const load: PageServerLoad = async (event) => {
	const params = paramsFrom((name) => event.url.searchParams.get(name));
	const { data, error: problem } = await api(event).POST('/api/v1/oauth/authorization', {
		body: { params }
	});
	if (problem || !data) {
		const message = problem
			? problemMessage(problem, 'Try again from the app.')
			: 'Try again from the app.';
		error(400, message);
	}
	// Problems the app should hear about go straight back to it.
	if (data.redirectTo) redirect(303, data.redirectTo);
	return { view: data, query: event.url.search };
};

async function decide(event: Parameters<Actions[string]>[0], approve: boolean) {
	const params = paramsFrom((name) => event.url.searchParams.get(name));
	const { data, error: problem } = await api(event).POST('/api/v1/oauth/authorization', {
		body: { params, approve }
	});
	if (problem || !data?.redirectTo) {
		return fail(400, {
			message: problem
				? problemMessage(problem, 'Try again from the app.')
				: 'Try again from the app.'
		});
	}
	// Not a redirect: the CSP's form-action 'self' would block a form post
	// redirecting to the app's address. The page navigates there instead.
	return { redirectTo: data.redirectTo, approved: approve };
}

export const actions: Actions = {
	approve: (event) => decide(event, true),
	deny: (event) => decide(event, false)
};
