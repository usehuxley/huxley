// `huxley workflows codegen` output (generated from fixtures/actions.json by
// the CLI's tests) type-checks and makes integration.action steps.
import { describe, expect, it } from 'vitest';
import { expr, ref } from '../src/index.ts';
import { cwAutomate, msgraph, type MsgraphListUsersOutput } from './fixtures/integrations.ts';

describe('generated integration builders', () => {
	it('make typed integration.action steps', () => {
		expect(msgraph.listUsers('users', { $top: 50, $filter: 'accountEnabled eq true' })).toEqual({
			id: 'users',
			type: 'integration.action',
			action: 'msgraph/list_users',
			with: { $top: 50, $filter: 'accountEnabled eq true' }
		});
		// Values may be expressions; step options pass through.
		const disable = msgraph.updateUser(
			'disable',
			{ id: ref.inputs.user, body: { accountEnabled: expr`${ref.inputs.enable} == true` } },
			{ connection: 'tenant', retry: { max_attempts: 3 } }
		);
		expect(disable).toMatchObject({
			action: 'msgraph/update_user',
			connection: 'tenant',
			with: { id: '{{ inputs.user }}', body: { accountEnabled: '{{ inputs.enable == true }}' } }
		});
		expect(cwAutomate.runCommand('run', { id: 12, body: { Command: 2 } }).action).toBe(
			'cw-automate/run_command'
		);
		const page: MsgraphListUsersOutput = { value: [{ id: 'a', displayName: null }] };
		expect(page.value?.[0].id).toBe('a');
	});
});
