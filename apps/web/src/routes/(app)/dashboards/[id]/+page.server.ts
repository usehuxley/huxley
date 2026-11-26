import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { api } from '#lib/server/api.ts';
import { answersFor, homeOrg } from '#lib/dashboards/server.ts';
import { RANGES } from '#lib/dashboards/spec.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data: dashboard, response } = await api(event).GET('/api/v1/dashboards/{id}', {
		params: { path: { id: event.params.id } }
	});
	if (!dashboard) error(response.status === 403 ? 403 : 404, 'There’s no such dashboard.');
	const asked = event.url.searchParams.get('range') ?? '';
	const range = RANGES.some((r) => r.value === asked) ? asked : dashboard.range;
	const orgId = event.url.searchParams.get('org') || null;
	const [{ results }, orgs] = await Promise.all([
		answersFor(event, dashboard, range, orgId),
		api(event).GET('/api/v1/orgs')
	]);
	const { user } = await event.parent();
	const canEdit =
		!dashboard.builtin &&
		(dashboard.ownerUserId === user.id || user.grantedPermissions.includes('dashboard:manage'));
	// Its author follows its publishing; shared ones name their audience.
	const mine =
		!dashboard.builtin && (dashboard.ownerUserId === user.id || dashboard.createdBy === user.id);
	const [submissions, audiences] = await Promise.all([
		mine
			? api(event).GET('/api/v1/dashboards/{id}/submissions', {
					params: { path: { id: dashboard.id } }
				})
			: Promise.resolve({ data: undefined }),
		mine ||
		user.grantedPermissions.includes('workflow:write') ||
		dashboard.audienceTeams.length + dashboard.audienceRoles.length > 0
			? api(event).GET('/api/v1/dashboard-audiences')
			: Promise.resolve({ data: undefined })
	]);
	return {
		submissions: submissions.data ?? [],
		audiences: audiences.data ?? { teams: [], roles: [] },
		canSubmit: dashboard.ownerUserId === user.id,
		// Personal dashboards can't be emailed on a schedule (it runs at an organization).
		canSchedule: user.grantedPermissions.includes('workflow:write') && !dashboard.ownerUserId,
		dashboard,
		range,
		orgId,
		results,
		orgs: (orgs.data ?? []).map((o) => ({ value: o.id, label: o.name })),
		canEdit
	};
};

export const actions: Actions = {
	copy: async (event) => {
		const { data: dashboard } = await api(event).GET('/api/v1/dashboards/{id}', {
			params: { path: { id: event.params.id } }
		});
		const orgId = await homeOrg(event);
		if (!dashboard || !orgId) return fail(404, { message: 'Could not copy it.' });
		const { data, response } = await api(event).POST('/api/v1/dashboards/{id}/copy', {
			params: { path: { id: event.params.id } },
			body: { orgId, name: `${dashboard.name} (copy)` }
		});
		if (!data) return fail(response.status, { message: 'Could not copy it.' });
		redirect(303, `/dashboards/${data.id}/edit`);
	},
	submit: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/dashboards/{id}/submit', {
			params: { path: { id: event.params.id } },
			body: {
				teams: form.getAll('teams').map(String),
				roles: form.getAll('roles').map(String),
				portal: form.get('portal') === 'on',
				note: formString(form, 'note') || null
			}
		});
		if (!data) {
			return fail(response.status, { publish: problemMessage(problem, 'Could not submit it.') });
		}
		return { submitted: true };
	},
	schedule: async (event) => {
		const form = await event.request.formData();
		const { data: dashboard } = await api(event).GET('/api/v1/dashboards/{id}', {
			params: { path: { id: event.params.id } }
		});
		const orgId = await homeOrg(event);
		if (!dashboard || !orgId) return fail(404, { schedule: 'Could not find the dashboard.' });
		const hour = Math.min(23, Math.max(0, Number(formString(form, 'hour')) || 8));
		const cron = {
			daily: `0 ${hour} * * *`,
			weekly: `0 ${hour} * * 1`,
			monthly: `0 ${hour} 1 * *`
		}[formString(form, 'cadence') as 'daily' | 'weekly' | 'monthly'];
		if (!cron) return fail(422, { schedule: 'Pick how often.' });
		const key = `report-${Date.now().toString(36)}`;
		const {
			data: workflow,
			error: problem,
			response
		} = await api(event).POST('/api/v1/workflows', {
			body: {
				orgId,
				document: {
					apiVersion: 'usehuxley.io/v1',
					kind: 'Workflow',
					metadata: {
						key,
						name: `Email “${dashboard.name}”`,
						description: `Emails the ${dashboard.name} dashboard (${formString(form, 'format')}) on a schedule.`
					},
					spec: {
						triggers: [{ type: 'schedule', cron }],
						steps: [
							{
								id: 'send',
								type: 'core.report.send',
								with: {
									dashboard_id: dashboard.id,
									format: formString(form, 'format') || 'pdf',
									to: formString(form, 'to')
								}
							}
						]
					}
				}
			}
		});
		if (!workflow) {
			return fail(response.status, { schedule: problemMessage(problem, 'Could not schedule it.') });
		}
		const published = await api(event).POST('/api/v1/workflows/{id}/publish', {
			params: { path: { id: workflow.id } },
			body: {}
		});
		if (published.response.status !== 200) {
			return fail(published.response.status, {
				schedule: 'Scheduled as a draft: publish it to start.'
			});
		}
		return { scheduled: workflow.id };
	},
	withdraw: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'submission');
		const { response } = await api(event).POST('/api/v1/dashboard-submissions/{id}/withdraw', {
			params: { path: { id } }
		});
		if (response.status !== 204)
			return fail(response.status, { message: 'Could not withdraw it.' });
		return { withdrawn: true };
	}
};
