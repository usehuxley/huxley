/**
 * What the visual editor knows about each HWF step type: how to label it,
 * what a new one looks like, which fields its panel edits, and which nested
 * blocks it holds (ADR-020, ADR-022).
 */

export type FieldKind = 'text' | 'expression' | 'number' | 'boolean' | 'json' | 'select' | 'action';

export interface FieldSpec {
	/** Dotted path within the step, e.g. `with.url`. */
	path: string;
	label: string;
	kind: FieldKind;
	hint?: string;
	options?: { value: string; label: string }[];
}

export interface StepSpec {
	type: string;
	label: string;
	description: string;
	group: 'Actions' | 'Data' | 'Control flow' | 'Finish';
	/** Fields of a new step, besides `id` and `type`. */
	template: Record<string, unknown>;
	fields: FieldSpec[];
	/** Nested step lists: fixed ones (`then`), or a list of lanes (`cases`). */
	blocks?: { path: string; label: string }[];
	lanes?: 'cases' | 'branches';
	/** The feature module the step belongs to; offered only while it's on. */
	module?: string;
}

const methods = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'].map((m) => ({
	value: m,
	label: m
}));

export const STEPS: StepSpec[] = [
	{
		type: 'http.request',
		label: 'HTTP request',
		description: 'Call an API.',
		group: 'Actions',
		template: { with: { method: 'GET', url: 'https://' } },
		fields: [
			{ path: 'with.method', label: 'Method', kind: 'select', options: methods },
			{ path: 'with.url', label: 'URL', kind: 'expression' },
			{ path: 'with.headers', label: 'Headers', kind: 'json', hint: 'An object' },
			{ path: 'with.query', label: 'Query', kind: 'json', hint: 'An object' },
			{ path: 'with.body', label: 'Body', kind: 'json' },
			{
				path: 'with.body_type',
				label: 'Body type',
				kind: 'select',
				options: [
					{ value: 'json', label: 'JSON' },
					{ value: 'form', label: 'Form' },
					{ value: 'text', label: 'Text' }
				]
			},
			{ path: 'with.allow_error_status', label: 'Treat 4xx/5xx as success', kind: 'boolean' }
		]
	},
	{
		type: 'script',
		label: 'Run script',
		description: 'Run a library script in a sandboxed runner.',
		group: 'Actions',
		template: { script: '', with: {}, timeout: '15m' },
		fields: [
			{ path: 'script', label: 'Script key', kind: 'text', hint: 'From Scripts, e.g. reset-mfa' },
			{ path: 'with', label: 'Parameters', kind: 'json', hint: 'An object' },
			{
				path: 'env',
				label: 'Environment',
				kind: 'json',
				hint: '{ "NAME": "{{ secrets.key }}" }; secrets are scrubbed from the output'
			},
			{
				path: 'connections',
				label: 'Connections',
				kind: 'json',
				hint: '{ "GRAPH": "msgraph" } gives GRAPH_URL and GRAPH_TOKEN'
			},
			{
				path: 'timeout',
				label: 'Timeout',
				kind: 'text',
				hint: 'Waiting for a runner included, e.g. 15m'
			}
		]
	},
	{
		type: 'core.email',
		label: 'Send email',
		description: 'Email any address through the instance’s mail server.',
		group: 'Actions',
		template: { with: { to: '', subject: '', text: '' } },
		fields: [
			{ path: 'with.to', label: 'To', kind: 'json', hint: 'An address, or a list (at most 20)' },
			{ path: 'with.subject', label: 'Subject', kind: 'expression' },
			{ path: 'with.text', label: 'Text', kind: 'expression' },
			{ path: 'with.html', label: 'HTML (optional)', kind: 'expression' }
		]
	},
	{
		type: 'dataset.query',
		label: 'Query a dataset',
		description: 'Read rows of a dataset: this organization’s and those below it.',
		group: 'Data',
		template: { with: { dataset: '' } },
		fields: [
			{ path: 'with.dataset', label: 'Dataset key', kind: 'text' },
			{
				path: 'with.filters',
				label: 'Filters',
				kind: 'json',
				hint: 'Column values rows must equal, e.g. { "licensed": true }'
			},
			{ path: 'with.org_id', label: 'Only this organization', kind: 'expression' },
			{
				path: 'with.order_by',
				label: 'Order by',
				kind: 'text',
				hint: 'A column; -column sorts descending'
			},
			{ path: 'with.limit', label: 'Limit', kind: 'expression', hint: 'Default 100, at most 1000' }
		]
	},
	{
		type: 'core.contact.find',
		label: 'Find contacts',
		description: 'Look contacts up by email or an external id, such as their Entra ID.',
		group: 'Data',
		module: 'crm',
		template: { with: { email: '' } },
		fields: [
			{ path: 'with.email', label: 'Email', kind: 'expression' },
			{
				path: 'with.external_id',
				label: 'External id',
				kind: 'json',
				hint: 'One system and id, e.g. { "entra": "<object id>" }'
			},
			{ path: 'with.org_id', label: 'Only this organization', kind: 'expression' }
		]
	},
	{
		type: 'core.contact.upsert',
		label: 'Add or update a contact',
		description: 'Update the contact with this email (or external id), or add them.',
		group: 'Data',
		module: 'crm',
		template: { with: { email: '', first_name: '', last_name: '' } },
		fields: [
			{ path: 'with.email', label: 'Email', kind: 'expression' },
			{ path: 'with.first_name', label: 'First name', kind: 'expression' },
			{ path: 'with.last_name', label: 'Last name', kind: 'expression' },
			{ path: 'with.job_title', label: 'Job title', kind: 'expression' },
			{ path: 'with.phone', label: 'Phone', kind: 'expression' },
			{ path: 'with.mobile', label: 'Mobile', kind: 'expression' },
			{
				path: 'with.external_ids',
				label: 'External ids',
				kind: 'json',
				hint: 'Merged into theirs, e.g. { "entra": "<object id>" }'
			},
			{
				path: 'with.match',
				label: 'Match on',
				kind: 'text',
				hint: 'email (default), or an external id’s system such as entra'
			},
			{ path: 'with.org_id', label: 'Organization', kind: 'expression', hint: 'Default: the run’s' }
		]
	},
	{
		type: 'core.company.get',
		label: 'Get company details',
		description: 'An organization’s company details and sites.',
		group: 'Data',
		module: 'crm',
		template: {},
		fields: [
			{ path: 'with.org_id', label: 'Organization', kind: 'expression', hint: 'Default: the run’s' }
		]
	},
	{
		type: 'core.company.update',
		label: 'Update company details',
		description: 'Change status, relationship, website, phone or notes; the rest stays.',
		group: 'Data',
		module: 'crm',
		template: { with: { status: 'active' } },
		fields: [
			{
				path: 'with.status',
				label: 'Status',
				kind: 'expression',
				hint: 'active, onboarding, offboarding or inactive'
			},
			{
				path: 'with.relationship',
				label: 'Relationship',
				kind: 'expression',
				hint: 'client, prospect, vendor or partner'
			},
			{ path: 'with.website', label: 'Website', kind: 'expression' },
			{ path: 'with.phone', label: 'Phone', kind: 'expression' },
			{ path: 'with.notes', label: 'Notes', kind: 'expression' },
			{ path: 'with.org_id', label: 'Organization', kind: 'expression', hint: 'Default: the run’s' }
		]
	},
	{
		type: 'core.ticket.create',
		label: 'Open a ticket',
		description: 'Open a ticket for an organization; the SLA applies as usual.',
		group: 'Data',
		module: 'servicedesk',
		template: { with: { title: '' } },
		fields: [
			{ path: 'with.title', label: 'Title', kind: 'expression' },
			{ path: 'with.description', label: 'Description', kind: 'expression' },
			{
				path: 'with.org_id',
				label: 'Organization',
				kind: 'expression',
				hint: 'Default: the run’s'
			},
			{
				path: 'with.type',
				label: 'Type',
				kind: 'text',
				hint: 'incident (default), service_request…'
			},
			{ path: 'with.impact', label: 'Impact', kind: 'expression', hint: '1 (high) to 3 (low)' },
			{ path: 'with.urgency', label: 'Urgency', kind: 'expression', hint: '1 (high) to 3 (low)' },
			{ path: 'with.queue_id', label: 'Queue', kind: 'expression' },
			{ path: 'with.requester_id', label: 'Requester', kind: 'expression', hint: 'A contact’s id' },
			{
				path: 'with.assignee_user_id',
				label: 'Assignee',
				kind: 'expression',
				hint: 'A person’s id'
			}
		]
	},
	{
		type: 'core.ticket.get',
		label: 'Get a ticket',
		description: 'Read a ticket, and its notes if asked.',
		group: 'Data',
		module: 'servicedesk',
		template: { with: { ticket_id: '' } },
		fields: [
			{ path: 'with.ticket_id', label: 'Ticket', kind: 'expression' },
			{ path: 'with.notes', label: 'With notes', kind: 'expression', hint: 'true to include them' }
		]
	},
	{
		type: 'core.ticket.find',
		label: 'Find tickets',
		description: 'Tickets here and below, by queue, assignee, state or search.',
		group: 'Data',
		module: 'servicedesk',
		template: { with: {} },
		fields: [
			{
				path: 'with.state',
				label: 'State',
				kind: 'text',
				hint: 'open (default), resolved, closed or all'
			},
			{ path: 'with.queue_id', label: 'Queue', kind: 'expression' },
			{
				path: 'with.assignee',
				label: 'Assignee',
				kind: 'expression',
				hint: 'A person’s id, or none'
			},
			{ path: 'with.search', label: 'Search', kind: 'expression', hint: 'Title or number' },
			{ path: 'with.org_id', label: 'Only this organization', kind: 'expression' },
			{ path: 'with.limit', label: 'Limit', kind: 'expression', hint: 'Default 50' }
		]
	},
	{
		type: 'core.ticket.update',
		label: 'Update a ticket',
		description: 'Change fields, assignment or status; what’s left out stays.',
		group: 'Data',
		module: 'servicedesk',
		template: { with: { ticket_id: '' } },
		fields: [
			{ path: 'with.ticket_id', label: 'Ticket', kind: 'expression' },
			{
				path: 'with.status',
				label: 'Status',
				kind: 'text',
				hint: 'A status key, e.g. in_progress'
			},
			{ path: 'with.resolution', label: 'Resolution', kind: 'expression' },
			{ path: 'with.assignee_user_id', label: 'Assignee', kind: 'expression' },
			{ path: 'with.queue_id', label: 'Queue', kind: 'expression' },
			{ path: 'with.priority', label: 'Priority', kind: 'text', hint: 'p1…p4, or auto' }
		]
	},
	{
		type: 'core.ticket.note',
		label: 'Add a ticket note',
		description: 'An internal (default), public or resolution note.',
		group: 'Data',
		module: 'servicedesk',
		template: { with: { ticket_id: '', body: '' } },
		fields: [
			{ path: 'with.ticket_id', label: 'Ticket', kind: 'expression' },
			{ path: 'with.body', label: 'Note', kind: 'expression' },
			{ path: 'with.kind', label: 'Kind', kind: 'text', hint: 'internal, public or resolution' }
		]
	},
	{
		type: 'core.ticket.dispatch',
		label: 'Dispatch a ticket',
		description: "Assign it by its queue's dispatch: round-robin, by load or by skills.",
		group: 'Data',
		module: 'servicedesk',
		template: { with: { ticket_id: '' } },
		fields: [
			{ path: 'with.ticket_id', label: 'Ticket', kind: 'expression' },
			{
				path: 'with.strategy',
				label: 'Strategy',
				kind: 'text',
				hint: "round_robin, load or skills (default: the queue's)"
			},
			{ path: 'with.reassign', label: 'Reassign if assigned', kind: 'expression' }
		]
	},
	{
		type: 'core.mail.fetch',
		label: 'Fetch mail',
		description:
			'Fetch new mail from IMAP and Microsoft 365 mailboxes; each new message raises email.received.',
		group: 'Data',
		module: 'servicedesk',
		template: {},
		fields: [
			{
				path: 'with.mailbox_id',
				label: 'Only this mailbox',
				kind: 'expression',
				hint: 'Default: every IMAP mailbox here and below'
			},
			{ path: 'with.limit', label: 'Limit', kind: 'expression', hint: 'Per mailbox; default 50' }
		]
	},
	{
		type: 'core.ticket.embed',
		label: 'Embed tickets',
		description:
			'Embed new and changed tickets with the embeddings provider, so they can be searched by meaning.',
		group: 'Data',
		module: 'servicedesk',
		template: {},
		fields: [
			{ path: 'with.limit', label: 'Limit', kind: 'expression', hint: 'Tickets a run; default 200' }
		]
	},
	{
		type: 'core.asset.sync',
		label: 'Sync assets',
		description: 'Run a dataset → asset mapping: rows become assets, matched and kept up to date.',
		group: 'Data',
		module: 'cmdb',
		template: { with: { mapping_id: '' } },
		fields: [
			{
				path: 'with.mapping_id',
				label: 'Mapping',
				kind: 'expression',
				hint: 'The mapping rule’s id (Settings → Asset sync)'
			}
		]
	},
	{
		type: 'core.asset.set_warranty',
		label: 'Record a warranty',
		description:
			'Record a warranty a vendor lookup found for an asset; the same reference is updated, not repeated.',
		group: 'Data',
		module: 'cmdb',
		template: { with: { asset_id: '', source: '', ends_on: '' } },
		fields: [
			{ path: 'with.asset_id', label: 'Asset', kind: 'expression', hint: 'The asset’s id' },
			{
				path: 'with.source',
				label: 'Lookup',
				kind: 'expression',
				hint: 'Its short name, e.g. dell'
			},
			{ path: 'with.ends_on', label: 'Ends on', kind: 'expression', hint: 'YYYY-MM-DD' },
			{ path: 'with.starts_on', label: 'Starts on', kind: 'expression', hint: 'YYYY-MM-DD' },
			{
				path: 'with.kind',
				label: 'Kind',
				kind: 'expression',
				hint: 'manufacturer, extended, support or other'
			},
			{ path: 'with.provider', label: 'Provider', kind: 'expression' },
			{
				path: 'with.reference',
				label: 'Reference',
				kind: 'expression',
				hint: 'Contract number or service tag'
			}
		]
	},
	{
		type: 'core.kb.embed',
		label: 'Embed articles',
		description:
			'Embed newly published knowledge base articles, so they can be searched by meaning and suggested on tickets.',
		group: 'Data',
		module: 'knowledge',
		template: {},
		fields: [
			{
				path: 'with.limit',
				label: 'Limit',
				kind: 'expression',
				hint: 'Articles a run; default 200'
			}
		]
	},
	{
		type: 'core.csat.survey',
		label: 'Ask how it went',
		description:
			'Ask a resolved ticket’s requester to rate it (by email with one-click links, and in the portal), unless their company opted out or sampling leaves it out.',
		group: 'Actions',
		module: 'cx',
		template: { with: { ticket: '{{ trigger.subject_id }}' } },
		fields: [{ path: 'with.ticket', label: 'Ticket', kind: 'expression', hint: 'The ticket’s id' }]
	},
	{
		type: 'core.nps.send',
		label: 'Ask the net promoter question',
		description:
			'Ask contacts here and below who raised a ticket lately (and weren’t asked lately) how likely they are to recommend you, by email and in the portal.',
		group: 'Actions',
		module: 'cx',
		template: {},
		fields: [
			{
				path: 'with.campaign',
				label: 'Round',
				kind: 'expression',
				hint: 'Default: the month, e.g. 2026-11'
			},
			{
				path: 'with.limit',
				label: 'At most',
				kind: 'expression',
				hint: 'Contacts a run; default 500'
			}
		]
	},
	{
		type: 'core.report.send',
		label: 'Email a report',
		description:
			'Email a shared or prebuilt dashboard, with this organization’s numbers, as a PDF, Excel or CSV attachment.',
		group: 'Actions',
		template: { with: { dashboard_id: 'service-desk', format: 'pdf', to: 'role:admin' } },
		fields: [
			{
				path: 'with.dashboard_id',
				label: 'Dashboard',
				kind: 'expression',
				hint: 'Its id, or a prebuilt key such as service-desk'
			},
			{
				path: 'with.format',
				label: 'Format',
				kind: 'select',
				options: [
					{ value: 'pdf', label: 'PDF' },
					{ value: 'xlsx', label: 'Excel (XLSX)' },
					{ value: 'csv', label: 'CSV' }
				]
			},
			{
				path: 'with.to',
				label: 'To',
				kind: 'expression',
				hint: 'user:<id>, team:<id> or role:<key>'
			},
			{
				path: 'with.range',
				label: 'Range',
				kind: 'expression',
				hint: 'e.g. last_7_days (default: the dashboard’s)'
			},
			{ path: 'with.subject', label: 'Subject', kind: 'expression' }
		]
	},
	{
		type: 'core.kb.draft',
		label: 'Draft a knowledge article',
		description:
			'Draft an article (tagged ai-draft) and submit it for review, linked to a ticket if you like.',
		group: 'Actions',
		module: 'knowledge',
		template: { with: { title: '', body: '', ticket_id: '{{ trigger.subject_id }}' } },
		fields: [
			{ path: 'with.title', label: 'Title', kind: 'expression' },
			{ path: 'with.body', label: 'Body (Markdown)', kind: 'expression' },
			{ path: 'with.ticket_id', label: 'Ticket', kind: 'expression' },
			{ path: 'with.submit', label: 'Submit for review', kind: 'boolean' }
		]
	},
	{
		type: 'core.time.suggest',
		label: 'Suggest time',
		description:
			'Suggest a time entry to whoever worked a ticket (unless they logged time on it); they log or dismiss it from My time.',
		group: 'Actions',
		module: 'servicedesk',
		template: { with: { ticket_id: '{{ trigger.subject_id }}', minutes: 15 } },
		fields: [
			{ path: 'with.ticket_id', label: 'Ticket', kind: 'expression' },
			{ path: 'with.minutes', label: 'Minutes', kind: 'expression' },
			{ path: 'with.notes', label: 'Notes', kind: 'expression' },
			{ path: 'with.user_id', label: 'Person', kind: 'expression', hint: 'Default: the assignee' }
		]
	},
	{
		type: 'core.ticket.insight',
		label: 'Record ticket insight',
		description:
			'Show a summary, the people, devices, applications and errors a ticket mentions, and a next step on the ticket.',
		group: 'Actions',
		module: 'servicedesk',
		template: {
			with: {
				ticket_id: '{{ trigger.subject_id }}',
				summary: '{{ steps.judge.output.summary }}',
				next_step: '{{ steps.judge.output.next_step }}'
			}
		},
		fields: [
			{ path: 'with.ticket_id', label: 'Ticket', kind: 'expression' },
			{ path: 'with.summary', label: 'Summary', kind: 'expression' },
			{
				path: 'with.entities',
				label: 'Entities',
				kind: 'json',
				hint: '{ people, devices, applications, errors }'
			},
			{ path: 'with.next_step', label: 'Next step', kind: 'expression' }
		]
	},
	{
		type: 'core.workforce.insights',
		label: 'Find skill gaps',
		description:
			'Find work few people are qualified for, recent tickets worked without their requirements, and certifications about to expire.',
		group: 'Data',
		module: 'workforce',
		template: { with: { days: 60 } },
		fields: [
			{
				path: 'with.days',
				label: 'Certifications expiring within (days)',
				kind: 'expression',
				hint: 'Default 60'
			}
		]
	},
	{
		type: 'core.dispatch.recommend',
		label: 'Recommend who takes it',
		description:
			'Rank who should take a ticket by skills, certifications, availability, load, history and familiarity with the client, with reasons.',
		group: 'Data',
		module: 'servicedesk',
		template: { with: { ticket_id: '{{ trigger.subject_id }}' } },
		fields: [
			{ path: 'with.ticket_id', label: 'Ticket', kind: 'expression' },
			{ path: 'with.limit', label: 'At most', kind: 'expression', hint: 'Default 5' }
		]
	},
	{
		type: 'core.triage.options',
		label: 'Triage options',
		description:
			'List the queues (with descriptions) and priorities a ticket can be triaged to, for an AI step.',
		group: 'Data',
		module: 'servicedesk',
		template: { with: { ticket_id: '{{ trigger.subject_id }}' } },
		fields: [
			{
				path: 'with.ticket_id',
				label: 'Ticket',
				kind: 'expression',
				hint: 'Leave empty for the run’s organization'
			}
		]
	},
	{
		type: 'core.triage.apply',
		label: 'Apply triage',
		description:
			'Record an AI triage answer and apply it while no one has worked the ticket (or record it in an accuracy evaluation).',
		group: 'Actions',
		module: 'servicedesk',
		template: {
			with: {
				ticket_id: '{{ trigger.subject_id }}',
				queue: '{{ steps.judge.output.queue }}',
				priority: '{{ steps.judge.output.priority }}',
				reason: '{{ steps.judge.output.reason }}'
			}
		},
		fields: [
			{ path: 'with.ticket_id', label: 'Ticket', kind: 'expression' },
			{ path: 'with.queue', label: 'Queue', kind: 'expression', hint: 'A queue name, or (none)' },
			{ path: 'with.priority', label: 'Priority', kind: 'expression', hint: 'p1 to p4' },
			{ path: 'with.reason', label: 'Reason', kind: 'expression' }
		]
	},
	{
		type: 'core.sentiment.record',
		label: 'Record sentiment',
		description:
			'Store how a client message sounds, -2 (very negative) to 2 (very positive), with a reason: it feeds client health.',
		group: 'Data',
		module: 'cx',
		template: {
			with: {
				ticket_id: '{{ trigger.subject_id }}',
				score: '{{ steps.judge.output.score }}',
				reason: '{{ steps.judge.output.reason }}'
			}
		},
		fields: [
			{ path: 'with.ticket_id', label: 'Ticket', kind: 'expression' },
			{
				path: 'with.note_id',
				label: 'Message',
				kind: 'expression',
				hint: 'A note id; leave empty for the ticket’s description'
			},
			{ path: 'with.score', label: 'Score', kind: 'expression', hint: '-2 (very negative) to 2' },
			{ path: 'with.reason', label: 'Reason', kind: 'expression' }
		]
	},
	{
		type: 'core.health.compute',
		label: 'Score client health',
		description:
			'Score each client here and below for today, from satisfaction, NPS, SLA misses, reopened tickets, ticket volume and renewals; declining clients raise an event.',
		group: 'Actions',
		module: 'cx',
		template: {},
		fields: []
	},
	{
		type: 'core.ticket.email_reply',
		label: 'Email a reply',
		description:
			'Email a public note to the ticket’s requester, threaded so their answer comes back to the ticket.',
		group: 'Data',
		module: 'servicedesk',
		template: { with: { note_id: '{{ inputs.change.note }}' } },
		fields: [
			{
				path: 'with.note_id',
				label: 'Note',
				kind: 'expression',
				hint: 'The note’s id, from ticket.note_added'
			}
		]
	},
	{
		type: 'core.ticket.from_email',
		label: 'Email to ticket',
		description:
			'Open a ticket from a received email, or add it to the ticket it replies to. Skips auto-replies and loops.',
		group: 'Data',
		module: 'servicedesk',
		template: { with: { email_id: '{{ inputs.email_id }}' } },
		fields: [
			{
				path: 'with.email_id',
				label: 'Email',
				kind: 'expression',
				hint: 'The email’s id, from email.received'
			}
		]
	},
	{
		type: 'dataset.upsert',
		label: 'Write to a dataset',
		description: 'Add or update rows (by the dataset’s key), or replace an organization’s rows.',
		group: 'Data',
		template: { with: { dataset: '', rows: '' } },
		fields: [
			{ path: 'with.dataset', label: 'Dataset key', kind: 'text' },
			{
				path: 'with.rows',
				label: 'Rows',
				kind: 'expression',
				hint: 'A list of objects of column values'
			},
			{
				path: 'with.org_id',
				label: 'Organization',
				kind: 'expression',
				hint: 'Default: this run’s organization'
			},
			{
				path: 'with.replace',
				label: 'Replace that organization’s rows',
				kind: 'expression',
				hint: 'true or false'
			}
		]
	},
	{
		type: 'ai.prompt',
		label: 'Ask AI',
		description: 'Send a prompt to your AI provider; the answer is text.',
		group: 'Actions',
		template: { with: { prompt: '' } },
		fields: [
			{ path: 'with.prompt', label: 'Prompt', kind: 'expression' },
			{ path: 'with.system', label: 'Instructions (optional)', kind: 'expression' },
			{
				path: 'with.model',
				label: 'Model (optional)',
				kind: 'text',
				hint: 'Default: the provider’s'
			}
		]
	},
	{
		type: 'ai.classify',
		label: 'Classify with AI',
		description: 'Have AI pick one of your labels for some text.',
		group: 'Actions',
		template: { with: { text: '', labels: [] } },
		fields: [
			{ path: 'with.text', label: 'Text', kind: 'expression' },
			{
				path: 'with.labels',
				label: 'Labels',
				kind: 'json',
				hint: 'e.g. ["billing", "outage", "other"]'
			},
			{ path: 'with.instructions', label: 'How to choose (optional)', kind: 'expression' },
			{ path: 'with.model', label: 'Model (optional)', kind: 'text' }
		]
	},
	{
		type: 'ai.extract',
		label: 'Extract with AI',
		description: 'Have AI pull structured data out of some text.',
		group: 'Actions',
		template: { with: { text: '', schema: { type: 'object', properties: {} } } },
		fields: [
			{ path: 'with.text', label: 'Text', kind: 'expression' },
			{
				path: 'with.schema',
				label: 'What to extract (JSON Schema)',
				kind: 'json',
				hint: '{ "type": "object", "properties": { "name": { "type": "string" } } }'
			},
			{ path: 'with.instructions', label: 'Instructions (optional)', kind: 'expression' },
			{ path: 'with.model', label: 'Model (optional)', kind: 'text' }
		]
	},
	{
		type: 'core.notify',
		label: 'Notify',
		description: 'Tell people in the app and by email.',
		group: 'Actions',
		template: { with: { to: 'role:admin', title: '' } },
		fields: [
			{
				path: 'with.to',
				label: 'To',
				kind: 'json',
				hint: 'user:<id>, team:<id>, role:<key>, or a list'
			},
			{ path: 'with.title', label: 'Title', kind: 'expression' },
			{ path: 'with.message', label: 'Message', kind: 'expression' },
			{ path: 'with.url', label: 'Link', kind: 'text', hint: 'A path in the app, e.g. /runs' },
			{
				path: 'with.kind',
				label: 'Kind',
				kind: 'text',
				hint: "For people's channel choices: workflow.notification (default), ticket.assigned, ticket.replied or ticket.sla"
			},
			{
				path: 'with.exclude',
				label: 'Except',
				kind: 'expression',
				hint: 'User ids not to notify, e.g. whoever made the change'
			}
		]
	},
	{
		type: 'integration.action',
		label: 'Integration action',
		description: 'Call an action of an imported integration.',
		group: 'Actions',
		template: { action: '', with: {} },
		fields: [
			{ path: 'action', label: 'Action', kind: 'action' },
			{
				path: 'connection',
				label: 'Connection key',
				kind: 'text',
				hint: 'Optional; by default the nearest connection to the integration'
			},
			{
				path: 'with',
				label: 'Inputs',
				kind: 'json',
				hint: 'Parameters by name; the request body as body'
			},
			{ path: 'all_pages', label: 'Follow every page', kind: 'boolean' },
			{
				path: 'max_pages',
				label: 'At most this many pages',
				kind: 'number',
				hint: '1 to 100; default 20'
			}
		]
	},
	{
		type: 'approval',
		label: 'Approval',
		description: 'Pause until someone approves or rejects.',
		group: 'Control flow',
		template: { title: '', approvers: ['role:admin'], expires: '7d' },
		fields: [
			{ path: 'title', label: 'Question', kind: 'expression' },
			{ path: 'message', label: 'Details', kind: 'expression' },
			{
				path: 'approvers',
				label: 'Approvers',
				kind: 'json',
				hint: 'user:<id>, team:<id>, role:<key>, contact:<id> or client_approvers (in the portal); the first decision counts'
			},
			{
				path: 'expires',
				label: 'Expires after',
				kind: 'text',
				hint: 'e.g. 3d; then the step fails'
			}
		]
	},
	{
		type: 'wait_for_callback',
		label: 'Wait for callback',
		description: 'Pause until another system calls a single-use URL.',
		group: 'Control flow',
		template: { expires: '24h' },
		fields: [
			{
				path: 'expires',
				label: 'Expires after',
				kind: 'text',
				hint: 'e.g. 2h; then the step fails'
			},
			{
				path: 'send',
				label: 'Send the URL',
				kind: 'json',
				hint: '{ "action": "rmm/runScript", "with": { "callback": "{{ callback.url }}" } } or { "request": { … } }'
			}
		]
	},
	{
		type: 'subworkflow',
		label: 'Run workflow',
		description: 'Run another workflow and use its outputs.',
		group: 'Actions',
		template: { workflow: '' },
		fields: [
			{ path: 'workflow', label: 'Workflow key', kind: 'text' },
			{ path: 'with', label: 'Inputs', kind: 'json', hint: 'An object' },
			{ path: 'wait', label: 'Wait for it to finish', kind: 'boolean' }
		]
	},
	{
		type: 'transform',
		label: 'Transform',
		description: 'Compute a value with an expression.',
		group: 'Data',
		template: { output: '{{ }}' },
		fields: [
			{
				path: 'output',
				label: 'Output',
				kind: 'json',
				hint: 'Any value; strings may be expressions'
			}
		]
	},
	{
		type: 'set_var',
		label: 'Set variables',
		description: 'Set run variables (run.vars).',
		group: 'Data',
		template: { vars: {} },
		fields: [{ path: 'vars', label: 'Variables', kind: 'json', hint: 'An object' }]
	},
	{
		type: 'wait',
		label: 'Wait',
		description: 'Pause for a while or until a time.',
		group: 'Control flow',
		template: { duration: '5m' },
		fields: [
			{ path: 'duration', label: 'Duration', kind: 'text', hint: 'e.g. 30s, 5m, 1h30m' },
			{ path: 'until', label: 'Until', kind: 'expression', hint: 'Or an RFC 3339 time' }
		]
	},
	{
		type: 'if',
		label: 'If',
		description: 'Branch on a condition.',
		group: 'Control flow',
		template: { condition: '{{ }}', then: [], else: [] },
		fields: [{ path: 'condition', label: 'Condition', kind: 'expression' }],
		blocks: [
			{ path: 'then', label: 'Then' },
			{ path: 'else', label: 'Else' }
		]
	},
	{
		type: 'switch',
		label: 'Switch',
		description: 'Choose a branch by value.',
		group: 'Control flow',
		template: { value: '{{ }}', cases: [{ when: '', steps: [] }], default: [] },
		fields: [{ path: 'value', label: 'Value', kind: 'expression' }],
		lanes: 'cases',
		blocks: [{ path: 'default', label: 'Otherwise' }]
	},
	{
		type: 'foreach',
		label: 'For each',
		description: 'Repeat steps for every item of a list.',
		group: 'Control flow',
		template: { items: '{{ }}', steps: [] },
		fields: [
			{ path: 'items', label: 'Items', kind: 'expression' },
			{ path: 'as', label: 'Item name', kind: 'text', hint: 'Default: item' },
			{ path: 'concurrency', label: 'At once', kind: 'number', hint: '1 to 50' }
		],
		blocks: [{ path: 'steps', label: 'For each item' }]
	},
	{
		type: 'parallel',
		label: 'Parallel',
		description: 'Run branches at the same time.',
		group: 'Control flow',
		template: {
			branches: [
				{ id: 'a', steps: [] },
				{ id: 'b', steps: [] }
			],
			join: 'all'
		},
		fields: [
			{
				path: 'join',
				label: 'Finish when',
				kind: 'select',
				options: [
					{ value: 'all', label: 'All branches finish' },
					{ value: 'any', label: 'The first branch succeeds' }
				]
			}
		],
		lanes: 'branches'
	},
	{
		type: 'try',
		label: 'Try',
		description: 'Handle errors from a group of steps.',
		group: 'Control flow',
		template: { steps: [], catch: [] },
		fields: [],
		blocks: [
			{ path: 'steps', label: 'Try' },
			{ path: 'catch', label: 'If it fails' },
			{ path: 'finally', label: 'Afterwards' }
		]
	},
	{
		type: 'return',
		label: 'Return',
		description: 'Finish the run with outputs.',
		group: 'Finish',
		template: { outputs: {} },
		fields: [{ path: 'outputs', label: 'Outputs', kind: 'json', hint: 'An object' }]
	},
	{
		type: 'fail',
		label: 'Fail',
		description: 'Stop the run with an error.',
		group: 'Finish',
		template: { message: '' },
		fields: [
			{ path: 'message', label: 'Message', kind: 'expression' },
			{ path: 'code', label: 'Code', kind: 'text' }
		]
	}
];

/** Fields every step has. */
export const COMMON_FIELDS: FieldSpec[] = [
	{ path: 'name', label: 'Display name', kind: 'text' },
	{ path: 'if', label: 'Only if', kind: 'expression', hint: 'Skip the step unless this is true' },
	{ path: 'timeout', label: 'Timeout', kind: 'text', hint: 'e.g. 30s' },
	{ path: 'retry', label: 'Retry', kind: 'json', hint: '{ "max_attempts": 3, "on": ["5xx"] }' },
	{ path: 'continue_on_error', label: 'Continue if it fails', kind: 'boolean' }
];

export function stepSpec(type: unknown): StepSpec | undefined {
	return STEPS.find((s) => s.type === type);
}

export type Step = Record<string, unknown>;
/** Where a value sits in the document: keys and indices from the root. */
export type Path = (string | number)[];

export function isStep(value: unknown): value is Step {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function getAt(root: unknown, path: Path): unknown {
	let current: unknown = root;
	for (const part of path) {
		if (current === null || typeof current !== 'object') return undefined;
		current = (current as Record<string | number, unknown>)[part];
	}
	return current;
}

/** Reads a dotted field path (`with.url`) inside a step. */
export function getField(step: Step, field: string): unknown {
	return getAt(step, field.split('.'));
}

/** Sets a dotted field path, creating objects on the way; `undefined` removes it. */
export function setField(step: Step, field: string, value: unknown): void {
	const parts = field.split('.');
	let current: Record<string, unknown> = step;
	for (const part of parts.slice(0, -1)) {
		if (!isStep(current[part])) current[part] = {};
		current = current[part] as Record<string, unknown>;
	}
	const last = parts[parts.length - 1];
	if (value === undefined || value === '') delete current[last];
	else current[last] = value;
}

/** Every step id in the document, to pick unique new ones. */
export function stepIds(document: unknown): Set<string> {
	const ids = new Set<string>();
	const visit = (value: unknown) => {
		if (Array.isArray(value)) value.forEach(visit);
		else if (isStep(value)) {
			if (typeof value.id === 'string' && typeof value.type === 'string') ids.add(value.id);
			Object.values(value).forEach(visit);
		}
	};
	visit(document);
	return ids;
}

/** A new step of `type` with an id not used elsewhere in the document. */
export function newStep(type: string, document: unknown): Step {
	const spec = stepSpec(type)!;
	const base = type.replace(/[^a-z]+/g, '_').replace(/^_+|_+$/g, '');
	const taken = stepIds(document);
	let n = 1;
	while (taken.has(`${base}_${n}`)) n++;
	return { id: `${base}_${n}`, type, ...structuredClone(spec.template) };
}

/** `spec.steps[0].then[1]` as a path, for matching diagnostics to steps. */
export function pathKey(path: Path): string {
	return path
		.map((part, i) => (typeof part === 'number' ? `[${part}]` : i === 0 ? part : `.${part}`))
		.join('');
}

/** A short line describing what a step does, for its card. */
export function summary(step: Step): string {
	const text = (v: unknown) =>
		typeof v === 'string' ? v : v === undefined ? '' : JSON.stringify(v);
	switch (step.type) {
		case 'http.request':
			return `${text(getField(step, 'with.method') ?? 'GET')} ${text(getField(step, 'with.url'))}`;
		case 'core.notify':
			return `${text(getField(step, 'with.to'))}: ${text(getField(step, 'with.title'))}`;
		case 'subworkflow':
			return text(step.workflow);
		case 'integration.action':
			return text(step.action);
		case 'approval':
			return text(step.title);
		case 'script':
			return text(step.script);
		case 'core.email':
			return text(isStep(step.with) ? step.with.subject : '');
		case 'ai.prompt':
			return text(isStep(step.with) ? step.with.prompt : '');
		case 'ai.classify':
		case 'ai.extract':
			return text(isStep(step.with) ? step.with.text : '');
		case 'dataset.query':
		case 'dataset.upsert':
			return text(isStep(step.with) ? step.with.dataset : '');
		case 'core.contact.find':
		case 'core.contact.upsert':
			return text(isStep(step.with) ? step.with.email : '');
		case 'core.company.update':
			return text(isStep(step.with) ? step.with.status : '');
		case 'core.ticket.create':
			return text(isStep(step.with) ? step.with.title : '');
		case 'core.ticket.update':
			return text(isStep(step.with) ? step.with.status : '');
		case 'core.ticket.note':
			return text(isStep(step.with) ? step.with.body : '');
		case 'core.ticket.dispatch':
			return text(isStep(step.with) ? (step.with.strategy ?? "by the queue's dispatch") : '');
		case 'core.ticket.from_email':
			return text(isStep(step.with) ? step.with.email_id : '');
		case 'wait_for_callback':
			return `expires after ${text(step.expires ?? '24h')}`;
		case 'transform':
			return text(step.output);
		case 'set_var':
			return Object.keys(isStep(step.vars) ? step.vars : {}).join(', ');
		case 'wait':
			return text(step.duration ?? step.until);
		case 'if':
			return text(step.condition);
		case 'switch':
			return text(step.value);
		case 'foreach':
			return text(step.items);
		case 'fail':
			return text(step.message);
		default:
			return '';
	}
}
