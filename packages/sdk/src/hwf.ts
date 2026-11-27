/* Generated from schemas/hwf/v1.json by `pnpm generate`. Do not edit: HWF v1 only grows (ADR-042). */

/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "DocumentKind".
 */
export type DocumentKind = 'Workflow';
/**
 * A step: the fields every step has, plus its type's own.
 *
 * Control-flow steps (`if`, `switch`, `foreach`, `parallel`, `try`) contain
 * nested step lists; there are no free-form edges (ADR-020).
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "Step".
 */
export type Step =
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			/**
			 * Any JSON value; strings may be expressions.
			 */
			output: unknown;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'transform';
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'set_var';
			vars: {
				[k: string]: unknown;
			};
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'http.request';
			with: HttpRequest;
	  }
	| {
			/**
			 * `<integration key>/<action key>`, e.g. `tickets-api/createTicket`.
			 */
			action: string;
			/**
			 * Follow every page (the action's pagination, set in the integration's
			 * overlay): the output is `{items, pages, truncated}`.
			 */
			all_pages?: boolean;
			/**
			 * A connection key. Default: the nearest connection to the
			 * integration that the run's organization can use.
			 */
			connection?: string | null;
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			/**
			 * The most pages followed with `all_pages` (default 20, at most 100).
			 */
			max_pages?: number | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'integration.action';
			/**
			 * The action's inputs: its parameters by name, and `body`.
			 */
			with?: unknown;
	  }
	| {
			condition: string;
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			else?: Step[];
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			then?: Step[];
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'if';
	  }
	| {
			cases: SwitchCase[];
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			default?: Step[];
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'switch';
			/**
			 * The expression to match.
			 */
			value: string;
	  }
	| {
			/**
			 * The loop variable's name (default `item`); `loop.index` is also set.
			 */
			as?: string | null;
			/**
			 * How many items run at once (default 1, at most 50).
			 */
			concurrency?: number | null;
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			/**
			 * An expression that evaluates to a list.
			 */
			items: string;
			name?: string | null;
			retry?: RetryPolicy | null;
			steps: Step[];
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'foreach';
	  }
	| {
			branches: Branch[];
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			/**
			 * When a `parallel` step finishes.
			 */
			join?: 'all' | 'any';
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'parallel';
	  }
	| {
			/**
			 * Run if a step in `steps` fails; `error` is in scope.
			 */
			catch?: Step[];
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Run afterwards either way.
			 */
			finally?: Step[];
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			steps: Step[];
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'try';
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * e.g. `10m`. Set this or `until`.
			 */
			duration?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'wait';
			/**
			 * An RFC 3339 timestamp, usually an expression. Set this or `duration`.
			 */
			until?: string | null;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'subworkflow';
			/**
			 * Wait for it and use its outputs (default), or start it and carry on.
			 */
			wait?: boolean;
			/**
			 * Its inputs.
			 */
			with?: unknown;
			/**
			 * The other workflow's key.
			 */
			workflow: string;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * How long the URL works, e.g. `2h` (default `24h`). After that the
			 * step fails with `kind: timeout`.
			 */
			expires?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Tells the other system where to call back, once the URL exists:
			 * `callback.url` is in scope here.
			 */
			send?: CallbackSend | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'wait_for_callback';
	  }
	| {
			/**
			 * Who may decide: `user:<id>`, `team:<id>`, `role:<key>` (holders at the
			 * run's organization), and client contacts in the portal:
			 * `contact:<id>` or `client_approvers` (the run's company's client
			 * approvers and admins). The first decision counts.
			 */
			approvers: unknown;
			/**
			 * Approvers may also ask for changes: the step's `decision` is then
			 * `approved`, `rejected` or `changes_requested`.
			 */
			changes?: boolean;
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * How long approvers have, e.g. `3d` (default `7d`). After that the
			 * step fails with `kind: timeout`.
			 */
			expires?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			/**
			 * More detail for approvers.
			 */
			message?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			/**
			 * What's being decided; shown to approvers.
			 */
			title: string;
			type: 'approval';
	  }
	| {
			/**
			 * Connections whose credentials the script gets as environment
			 * variables: `NAME: <integration>` or `NAME: <integration>/<connection>`
			 * gives `NAME_URL` and `NAME_TOKEN` (OAuth 2.0, bearer), `NAME_KEY`
			 * (API key) or `NAME_USERNAME` and `NAME_PASSWORD` (basic), fetched
			 * when the script starts and scrubbed from its output and logs.
			 */
			connections?: {
				[k: string]: string;
			};
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Environment variables for the script (names in upper case), e.g.
			 * from variables or secrets. Secret values are scrubbed from its
			 * output and logs.
			 */
			env?: {
				[k: string]: unknown;
			};
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * The library script's key (this organization's or the nearest one
			 * above that has it).
			 */
			script: string;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'script';
			/**
			 * Its parameters.
			 */
			with?: unknown;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			outputs?: {
				[k: string]: unknown;
			} | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'return';
	  }
	| {
			code?: string | null;
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			message: string;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'fail';
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.notify';
			with: Notification;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.email';
			with: EmailMessage;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'dataset.query';
			with: DatasetQuery;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'dataset.upsert';
			with: DatasetUpsert;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.contact.find';
			with: ContactFind;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.contact.upsert';
			with: ContactUpsert;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.company.get';
			with?: CompanyRef;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.company.update';
			with: CompanyUpdate;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.ticket.from_email';
			with: TicketFromEmail;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.ticket.create';
			with: TicketCreate;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.ticket.get';
			with: TicketGet;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.ticket.find';
			with: TicketFind;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.ticket.update';
			with: TicketUpdate;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.ticket.note';
			with: TicketNote;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.ticket.dispatch';
			with: TicketDispatch;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.mail.fetch';
			with?: MailFetch;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.ticket.embed';
			with?: TicketEmbed;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.kb.embed';
			with?: KbEmbed;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.timesheet.check';
			with: TimesheetCheck;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.csat.survey';
			with: CsatSurvey;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.nps.send';
			with?: NpsSend;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.health.compute';
			with?: HealthCompute;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.sentiment.record';
			with: SentimentRecord;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.triage.options';
			with?: TriageOptions;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.triage.apply';
			with: TriageApply;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.dispatch.recommend';
			with: DispatchRecommend;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.workforce.insights';
			with?: WorkforceInsights;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.ticket.insight';
			with: TicketInsight;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.kb.draft';
			with: KbDraft;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.time.suggest';
			with: TimeSuggest;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.report.send';
			with: ReportSend;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.change.decide';
			with: ChangeDecide;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.project.scope.decide';
			with: ScopeDecide;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.quote.review';
			with: QuoteReview;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.sales.log_email';
			with: SalesLogEmail;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.calendar.sync';
			with?: CalendarSync | null;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.calendar.write';
			with: CalendarWrite;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.project.status';
			with?: ProjectStatus | null;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.project.report';
			with: ProjectReport;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.asset.sync';
			with: AssetSync;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.asset.set_warranty';
			with: AssetWarranty;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'core.ticket.email_reply';
			with: TicketEmailReply;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'ai.prompt';
			with: AiPrompt;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'ai.classify';
			with: AiClassify;
	  }
	| {
			/**
			 * Record the error as the step's output and carry on.
			 */
			continue_on_error?: boolean;
			description?: string | null;
			/**
			 * Unique in the workflow: lowercase letters, digits, underscores.
			 * Other steps read this step's output as `steps.<id>.output`.
			 */
			id: string;
			/**
			 * Skip the step unless this expression is true.
			 */
			if?: string | null;
			name?: string | null;
			retry?: RetryPolicy | null;
			/**
			 * Fail the attempt after this long, e.g. `30s`.
			 */
			timeout?: string | null;
			type: 'ai.extract';
			with: AiExtract;
	  };
/**
 * What starts a run.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "Trigger".
 */
export type Trigger =
	| {
			id?: string | null;
			type: 'manual';
	  }
	| {
			/**
			 * Five-field cron expression. Set this or `interval`.
			 */
			cron?: string | null;
			id?: string | null;
			/**
			 * Inputs for each scheduled run.
			 */
			inputs?: unknown;
			/**
			 * e.g. `15m`. Set this or `cron`.
			 */
			interval?: string | null;
			/**
			 * IANA time zone for `cron`; defaults to the organization's.
			 */
			timezone?: string | null;
			type: 'schedule';
	  }
	| {
			auth?: WebhookAuth | null;
			id?: string | null;
			response?: 'async' | 'sync';
			/**
			 * How long a `sync` webhook waits for the run, e.g. `30s` (at most 5m).
			 */
			timeout?: string | null;
			type: 'webhook';
	  }
	| {
			/**
			 * e.g. `ticket.created`.
			 */
			event: string;
			/**
			 * Start a run only when this expression is true. `event` is in scope.
			 */
			filter?: string | null;
			id?: string | null;
			type: 'event';
	  }
	| {
			/**
			 * The list action: `<integration key>/<action key>`.
			 */
			action: string;
			/**
			 * A connection key; default: the nearest one, as for action steps.
			 */
			connection?: string | null;
			id?: string | null;
			/**
			 * How often to check, e.g. `5m` (at least `1m`).
			 */
			interval: string;
			/**
			 * The field that identifies an item (dotted path; default `id`).
			 */
			item_id?: string | null;
			/**
			 * Where the list is in the response, as a dotted path (e.g. `value`);
			 * by default the response is the list.
			 */
			items?: string | null;
			/**
			 * Start runs for new items only (default), or for changed ones too.
			 */
			on?: 'new' | 'new_or_changed';
			type: 'poll';
			/**
			 * The action's inputs, passed as they are (not expressions).
			 */
			with?: unknown;
	  }
	| {
			/**
			 * The form's key.
			 */
			form: string;
			id?: string | null;
			type: 'form';
	  }
	| {
			/**
			 * What the tool does, for the model. Defaults to the workflow's description.
			 */
			description?: string | null;
			id?: string | null;
			type: 'mcp_tool';
	  };
/**
 * How webhook callers prove who they are. Secrets are expressions, usually
 * a secret organization variable: `{{ vars.webhook_secret }}`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "WebhookAuth".
 */
export type WebhookAuth =
	| {
			algorithm?: 'sha256' | 'sha512' | 'sha1';
			header?: string;
			/**
			 * Text before the hex digest, e.g. `sha256=`.
			 */
			prefix?: string | null;
			secret: string;
			/**
			 * A signed timestamp header; requests older than 5 minutes are refused.
			 */
			timestamp_header?: string | null;
			type: 'hmac';
	  }
	| {
			token: string;
			type: 'bearer';
	  }
	| {
			password: string;
			type: 'basic';
			username: string;
	  };
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "Backoff".
 */
export type Backoff = ('fixed' | 'exponential') | 'jitter';
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "BodyType".
 */
export type BodyType = 'json' | 'form' | 'text';
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "HmacAlgorithm".
 */
export type HmacAlgorithm = 'sha256' | 'sha512' | 'sha1';
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "HttpMethod".
 */
export type HttpMethod = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE' | 'HEAD' | 'OPTIONS';
/**
 * When a `parallel` step finishes.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "Join".
 */
export type Join = 'all' | 'any';
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "PollOn".
 */
export type PollOn = 'new' | 'new_or_changed';
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "WebhookResponse".
 */
export type WebhookResponse = 'async' | 'sync';

/**
 * A workflow document.
 */
export interface HuxleyWorkflowFormatV1 {
	/**
	 * Always `usehuxley.io/v1`.
	 */
	apiVersion: string;
	kind: DocumentKind;
	metadata: Metadata;
	spec: Spec;
	/**
	 * Canvas layout (node positions, notes). Ignored by the engine, so code
	 * edits and canvas edits don't conflict.
	 */
	ui?: unknown;
}
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "Metadata".
 */
export interface Metadata {
	description?: string | null;
	/**
	 * Stable identifier, unique per owning organization: lowercase letters,
	 * digits and hyphens.
	 */
	key: string;
	name: string;
	tags?: string[];
}
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "Spec".
 */
export interface Spec {
	/**
	 * A JSON Schema (type `object`) for the run's inputs. It also drives the
	 * generated form for manual runs.
	 */
	inputs?: unknown;
	/**
	 * Steps run when the run fails and nothing caught the error. `error` is
	 * in scope.
	 */
	on_error?: Step[];
	/**
	 * The run's outputs, usually expressions over step outputs.
	 */
	outputs?: {
		[k: string]: unknown;
	} | null;
	settings?: Settings | null;
	steps: Step[];
	triggers?: Trigger[];
}
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "RetryPolicy".
 */
export interface RetryPolicy {
	backoff?: ('fixed' | 'exponential') | 'jitter';
	/**
	 * Delay before the first retry (default `1s`).
	 */
	delay?: string | null;
	/**
	 * Attempts in total, including the first (2 to 20).
	 */
	max_attempts: number;
	/**
	 * Longest delay between attempts (default `5m`).
	 */
	max_delay?: string | null;
	/**
	 * Which failures to retry: HTTP statuses (`429`), classes (`5xx`), or
	 * `timeout` / `network`. Empty retries every failure.
	 */
	on?: string[];
}
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "HttpRequest".
 */
export interface HttpRequest {
	/**
	 * Treat 4xx/5xx responses as success (they fail the step by default).
	 */
	allow_error_status?: boolean;
	/**
	 * Sent as JSON, unless `body_type` says otherwise.
	 */
	body?: unknown;
	body_type?: 'json' | 'form' | 'text';
	headers?: {
		[k: string]: string;
	};
	method?: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE' | 'HEAD' | 'OPTIONS';
	query?: {
		[k: string]: unknown;
	};
	url: string;
}
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "SwitchCase".
 */
export interface SwitchCase {
	steps: Step[];
	/**
	 * Compared for equality with the switch's `value`.
	 */
	when: unknown;
}
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "Branch".
 */
export interface Branch {
	/**
	 * Unique within the step.
	 */
	id: string;
	steps: Step[];
}
/**
 * How a `wait_for_callback` step hands its URL over: an integration action
 * or a plain HTTP request (set one).
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "CallbackSend".
 */
export interface CallbackSend {
	/**
	 * `<integration key>/<action key>`, as in `integration.action`.
	 */
	action?: string | null;
	connection?: string | null;
	/**
	 * Or an HTTP request, as in `http.request`.
	 */
	request?: HttpRequest | null;
	/**
	 * The action's inputs.
	 */
	with?: unknown;
}
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "Notification".
 */
export interface Notification {
	/**
	 * People not to notify (user ids), e.g. whoever made the change.
	 */
	exclude?: unknown;
	/**
	 * The kind of notification, for people's channel preferences:
	 * `workflow.notification` (the default), `ticket.assigned`,
	 * `ticket.replied` or `ticket.sla`.
	 */
	kind?: string | null;
	message?: string | null;
	title: string;
	/**
	 * Who: `user:<id>`, `team:<id>`, `role:<key>`, or a list of those.
	 */
	to: unknown;
	/**
	 * A link in the app.
	 */
	url?: string | null;
}
/**
 * An email to send (`core.email`).
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "EmailMessage".
 */
export interface EmailMessage {
	/**
	 * An HTML body, sent alongside the text.
	 */
	html?: string | null;
	subject: string;
	/**
	 * The plain-text body.
	 */
	text: string;
	/**
	 * An address or a list of them (at most 20); each gets their own copy.
	 */
	to: unknown;
}
/**
 * Rows to read (`dataset.query`). Output: `{ rows, total }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "DatasetQuery".
 */
export interface DatasetQuery {
	/**
	 * The dataset's key: the run's organization's, or the nearest above.
	 */
	dataset: string;
	/**
	 * Column values rows must equal.
	 */
	filters?: unknown;
	/**
	 * At most this many rows (default 100, at most 1000).
	 */
	limit?: unknown;
	/**
	 * A column to sort by; `-name` sorts descending.
	 */
	order_by?: string | null;
	/**
	 * Only this organization's rows (it must be the run's or below it).
	 */
	org_id?: string | null;
}
/**
 * Rows to write (`dataset.upsert`). Output: `{ written }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "DatasetUpsert".
 */
export interface DatasetUpsert {
	dataset: string;
	/**
	 * Build each row from an item of `rows`: column name → dot path into
	 * the item (e.g. `upn: userPrincipalName`, `city: address.city`).
	 */
	map?: unknown;
	/**
	 * The organization the rows belong to (default: the run's).
	 */
	org_id?: string | null;
	/**
	 * Replace all of that organization's rows (a full refresh).
	 */
	replace?: unknown;
	/**
	 * A list of rows (objects of column values), usually an expression.
	 */
	rows: unknown;
}
/**
 * Contacts to find (`core.contact.find`): by `email`, by `external_id`
 * (e.g. `{ entra: "<object id>" }`), or both. Among the run's organization
 * and those below it, or `org_id` and those below it. Output:
 * `{ contacts, total }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "ContactFind".
 */
export interface ContactFind {
	email?: unknown;
	/**
	 * One external id: `{ <system>: <id> }`.
	 */
	external_id?: unknown;
	org_id?: string | null;
}
/**
 * A contact to create or update (`core.contact.upsert`). It's matched by
 * `match` (`email`, the default, or an external id's system, e.g.
 * `entra`) among the contacts of `org_id` (default: the run's
 * organization); a match is updated with the fields given, otherwise a
 * contact is created there. Output: `{ contact, created }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "ContactUpsert".
 */
export interface ContactUpsert {
	email?: unknown;
	/**
	 * Identities elsewhere, merged into the contact's: `{ entra: "…" }`.
	 */
	external_ids?: unknown;
	first_name?: unknown;
	/**
	 * At the contact's primary company.
	 */
	job_title?: unknown;
	last_name?: unknown;
	/**
	 * What identifies the contact: `email` or an external id's system.
	 */
	match?: unknown;
	mobile?: unknown;
	org_id?: string | null;
	phone?: unknown;
}
/**
 * Which organization (`core.company.get`): `org_id`, default the run's.
 * Output: `{ company, sites }`.
 */
export interface CompanyRef {
	org_id?: string | null;
}
/**
 * Company details to change (`core.company.update`); fields left out stay
 * as they are. Output: `{ company }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "CompanyUpdate".
 */
export interface CompanyUpdate {
	notes?: unknown;
	org_id?: string | null;
	phone?: unknown;
	/**
	 * `client`, `vendor`, `partner` or `prospect`.
	 */
	relationship?: unknown;
	/**
	 * `active`, `inactive`, `onboarding` or `offboarding`.
	 */
	status?: unknown;
	website?: unknown;
}
/**
 * A received email to turn into ticket work (`core.ticket.from_email`):
 * skipped if it's automatic mail or a loop, added as a reply to the ticket
 * it belongs to (by its `[HX#…]` subject token or the messages it answers;
 * a resolved ticket reopens), or opened as a new ticket for its sender, per
 * its mailbox's rules. Its attachments go on the ticket. Output:
 * `{ outcome, detail, ticket_id, created }`; `outcome` is `ticket_created`,
 * `note_added`, `ticket_reopened`, `ignored_automatic`, `ignored_loop` or
 * `ignored_unknown_sender`. Running it again for the same email changes
 * nothing.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TicketFromEmail".
 */
export interface TicketFromEmail {
	/**
	 * The email's id (from `email.received`).
	 */
	email_id: unknown;
}
/**
 * A ticket to open (`core.ticket.create`): at `org_id` (default: the run's
 * organization), with `title` and optionally `description`, `type` (a
 * ticket type's key, default `incident`), `impact` and `urgency` (1 high …
 * 3 low), `priority` (`p1`…`p4`, instead of the matrix), `queue_id`,
 * `requester_id` (a contact there), `assignee_user_id`,
 * `assignee_team_id` and `source` (default `workflow`). The SLA applies as
 * usual. Output: `{ ticket }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TicketCreate".
 */
export interface TicketCreate {
	assignee_team_id?: unknown;
	assignee_user_id?: unknown;
	description?: unknown;
	impact?: unknown;
	org_id?: string | null;
	priority?: unknown;
	queue_id?: unknown;
	requester_id?: unknown;
	source?: unknown;
	title: unknown;
	type?: unknown;
	urgency?: unknown;
}
/**
 * A ticket to read (`core.ticket.get`), with its notes if `notes` is
 * true. Output: `{ ticket, notes }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TicketGet".
 */
export interface TicketGet {
	notes?: unknown;
	ticket_id: unknown;
}
/**
 * Tickets to find (`core.ticket.find`) among the run's organization and
 * those below it (or `org_id`'s): by `queue_id`, `assignee` (a person's
 * id, or `none`), `state` (`open`, the default, `resolved`, `closed` or
 * `all`), `search` (title or number) and `priority` (e.g. `p1,p2`), most
 * urgent first, at most
 * `limit` (default 50). Output: `{ tickets, total }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TicketFind".
 */
export interface TicketFind {
	assignee?: unknown;
	limit?: unknown;
	org_id?: string | null;
	/**
	 * Only these priorities, comma-separated: e.g. `p1,p2`.
	 */
	priority?: unknown;
	queue_id?: unknown;
	search?: unknown;
	state?: unknown;
}
/**
 * Changes to a ticket (`core.ticket.update`): any of `title`,
 * `description`, `impact`, `urgency`, `priority` (or `auto`), and
 * `queue_id`, `requester_id`, `assignee_user_id`, `assignee_team_id`
 * (`null` clears); then `status` (a status's key), with `resolution` where
 * the status needs one. Applied to the ticket as it is now. Output:
 * `{ ticket }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TicketUpdate".
 */
export interface TicketUpdate {
	assignee_team_id?: unknown;
	assignee_user_id?: unknown;
	description?: unknown;
	impact?: unknown;
	priority?: unknown;
	queue_id?: unknown;
	requester_id?: unknown;
	resolution?: unknown;
	status?: unknown;
	ticket_id: unknown;
	title?: unknown;
	urgency?: unknown;
}
/**
 * A note to add (`core.ticket.note`): `body`, and `kind`: `internal` (the
 * default), `public` or `resolution`. Output: `{ note }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TicketNote".
 */
export interface TicketNote {
	body: unknown;
	kind?: unknown;
	ticket_id: unknown;
}
/**
 * A ticket to assign (`core.ticket.dispatch`) by its queue's strategy
 * (`round_robin`, `load` or `skills`), or `strategy` to use another. With
 * the `workforce` module on, people missing a required certification, away
 * or outside their hours are skipped. A ticket that's already assigned is
 * left alone unless `reassign` is true. Output: `{ assigned, assignee,
 * strategy, reason, considered }`; `reason` says why nobody was chosen
 * (`assigned`, `no_queue`, `manual`, `nobody_available`).
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TicketDispatch".
 */
export interface TicketDispatch {
	reassign?: unknown;
	strategy?: unknown;
	ticket_id: unknown;
}
/**
 * Mail to fetch (`core.mail.fetch`): new messages from the enabled IMAP
 * and Microsoft 365 mailboxes at the run's organization and below, or just `mailbox_id`, up
 * to `limit` each (default 50). Each new message is kept and announced with
 * `email.received`. A mailbox that can't be reached is reported (and shown
 * on it) without failing the step. Output: `{ mailboxes: [{ mailbox_id,
 * address, fetched, new } | { mailbox_id, address, error }] }`.
 */
export interface MailFetch {
	limit?: unknown;
	mailbox_id?: unknown;
}
/**
 * Tickets to embed (`core.ticket.embed`): those at the run's organization
 * and below whose embedding is missing or stale, newest changes first, up
 * to `limit` (default 200), by each organization's embeddings provider.
 * Does nothing when pgvector isn't installed (search by meaning is off). A
 * provider that fails is reported without failing the step. Output:
 * `{ available, embedded, errors: [{ org_id, error }] }`.
 */
export interface TicketEmbed {
	limit?: unknown;
}
/**
 * Articles to embed (`core.kb.embed`): published knowledge base articles
 * at the run's organization and below whose embedding is missing or stale
 * (their published title or text changed, they moved organization, or the
 * embeddings provider's model changed), up to `limit` (default 200), by
 * each organization's embeddings provider. Does nothing when pgvector
 * isn't installed. A provider that fails is reported without failing the
 * step. Output: `{ available, embedded, errors: [{ org_id, error }] }`.
 */
export interface KbEmbed {
	limit?: unknown;
}
/**
 * What to look for in timesheets (`core.timesheet.check`), for reminders:
 *
 * - `unsubmitted`: the last period that ended at least `days_late` days ago
 *   (default 1), for people who log time or have a working schedule,
 *   whose timesheet for it is open or rejected.
 * - `under_logged`: a day (`date`, `YYYY-MM-DD`; default yesterday) people
 *   logged less than their schedule expects, not on time off.
 * - `missing_notes`: entries without notes in the current period.
 *
 * Output: `{ check, people: [{ user_id, person, email, period_start,
 * period_end, status, logged_minutes, expected_minutes, missing_notes,
 * days_late, leads: [user ids], recipient, lead_recipients }] }`, where
 * `recipient` and `lead_recipients` are ready for `core.notify`'s `to`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TimesheetCheck".
 */
export interface TimesheetCheck {
	check: unknown;
	date?: unknown;
	days_late?: unknown;
}
/**
 * A satisfaction survey (`core.csat.survey`) for a resolved ticket: unless
 * its company opted out, it has no requester, they were asked about it in
 * the last week, or sampling leaves it out, the requester is asked (by
 * email with one-click rating links when email and the portal's address
 * are set up, and in the portal). Output: `{ sent, skipped, survey,
 * emailed }`, where `skipped` says why not (`opted_out`, `no_requester`,
 * `already_asked`, `sampled_out`, `not_resolved`).
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "CsatSurvey".
 */
export interface CsatSurvey {
	/**
	 * The ticket's id.
	 */
	ticket: unknown;
}
/**
 * A net promoter round (`core.nps.send`): contacts in the run's
 * organization and below who raised a ticket lately and weren't asked
 * lately (each company's Customer experience settings; companies can opt
 * out) are asked how likely they are to recommend you, 0 to 10, by email
 * when email and the portal's address are set up, and in the portal.
 * `campaign` names the round (default: the month, e.g. `2026-11`); `limit`
 * caps how many are asked (default 500). Output: `{ asked, emailed,
 * campaign }`.
 */
export interface NpsSend {
	campaign?: unknown;
	limit?: unknown;
}
/**
 * Client health for today (`core.health.compute`): each client company in
 * the run's organization and below gets a score, 0 to 100, from weighted
 * factors (satisfaction, NPS, SLA misses, reopened tickets, ticket volume,
 * renewals, sentiment), with the reasons; those that dropped 10 points or
 * more from a week before emit `client.health_declining`, and those whose
 * messages turned negative emit `client.sentiment_declining` (at most
 * weekly). It takes nothing. Output: `{ companies, declining: [{ org_id,
 * name, score, previous, reasons }], sentiment_falling: [{ org_id, name,
 * recent, previous, explanations }] }`.
 */
export interface HealthCompute {}
/**
 * A client message's sentiment to record (`core.sentiment.record`): the
 * ticket (in the run's organization or below), the note judged (none: the
 * ticket's description), `score` from -2 (very negative) to 2 (very
 * positive), and a one-line `reason`. Again for the same message replaces
 * it. Output: `{ id, org_id, ticket_id, score }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "SentimentRecord".
 */
export interface SentimentRecord {
	note_id?: unknown;
	reason: unknown;
	score: unknown;
	ticket_id: unknown;
}
/**
 * What a ticket can be triaged to (`core.triage.options`): the queues at
 * `ticket_id`'s organization or above (or the run's organization's,
 * without one), and the priorities. Output: `{ queues: [{ id, name,
 * description }], queue_names (the names and "(none)"), priority_keys,
 * skill_names (the workforce skill catalog's and "(none)"), guide (a line
 * per queue, priority and skill, for a prompt) }`.
 */
export interface TriageOptions {
	ticket_id?: unknown;
}
/**
 * A triage answer to record (`core.triage.apply`): `queue` (a name from
 * `core.triage.options`, or "(none)"), `priority` (`p1` to `p4`) and a
 * `reason`, and the `skills` it needs. With `ticket_id`, it's kept as the
 * ticket's suggestion and applied while no person has changed the ticket
 * and the organization lets triage apply (the queue only if it has none;
 * skills as the ticket's own requirements, at working level, if it has
 * none). With `evaluation_id` and `sample_id` (an accuracy
 * evaluation's run), it's recorded in the evaluation instead. Output: `{
 * mode, applied, queue_id, priority }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TriageApply".
 */
export interface TriageApply {
	evaluation_id?: unknown;
	priority: unknown;
	queue: unknown;
	reason?: unknown;
	sample_id?: unknown;
	/**
	 * The skills the ticket needs, by name (from `skill_names`).
	 */
	skills?: unknown;
	ticket_id?: unknown;
}
/**
 * Who should take a ticket (`core.dispatch.recommend`), in the run's
 * organization or below: its queue's members (or those who may work
 * tickets there), ranked by required skills and certifications,
 * availability, open load, history in the queue and familiarity with the
 * client, at most `limit` (default 5). Output: `{ recommendations: [{
 * userId, name, score, qualified, available, reasons, cautions, … }], top
 * }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "DispatchRecommend".
 */
export interface DispatchRecommend {
	limit?: unknown;
	ticket_id: unknown;
}
/**
 * Skill gaps now (`core.workforce.insights`): who meets each queue's and
 * ticket type's requirements, recent tickets worked without them, and
 * certifications expiring within `days` (default 60) with the work only
 * their holder is qualified for. Output: `{ coverage, unqualified,
 * expiring, summary }` (`summary`: a line per finding, most serious
 * first).
 */
export interface WorkforceInsights {
	days?: unknown;
}
/**
 * A ticket's insight to record (`core.ticket.insight`), shown on the
 * ticket: a `summary`, the `entities` it mentions (`{ people, devices,
 * applications, errors }`, lists of names) and a `next_step`. Again
 * replaces it. Output: the insight.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TicketInsight".
 */
export interface TicketInsight {
	entities?: unknown;
	next_step?: unknown;
	summary: unknown;
	ticket_id: unknown;
}
/**
 * A knowledge article to draft (`core.kb.draft`): its `title` and `body`
 * (Markdown), at `org_id` (default the run's organization), tagged
 * `ai-draft`, submitted for review unless `submit` is false, and linked
 * to `ticket_id` if given. Output: `{ article_id, status }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "KbDraft".
 */
export interface KbDraft {
	body: unknown;
	org_id?: unknown;
	submit?: unknown;
	ticket_id?: unknown;
	title: unknown;
}
/**
 * A time entry to suggest (`core.time.suggest`): `minutes` on `ticket_id`
 * for `user_id` (default the ticket's assignee) with `notes`, unless they
 * logged time on it already or a suggestion is waiting. They accept it
 * (a real entry) or dismiss it from My time. Output: `{ suggestion }`
 * (null: skipped).
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TimeSuggest".
 */
export interface TimeSuggest {
	minutes: unknown;
	notes?: unknown;
	ticket_id: unknown;
	user_id?: unknown;
}
/**
 * A report to email (`core.report.send`): a shared dashboard (at the
 * run's organization, above or below it) or a prebuilt one by key, with
 * the numbers of the run's organization and below over `range` (default:
 * the dashboard's), as `format` (`csv`, `xlsx` or `pdf`, the default),
 * attached to an email to `to` (`user:<id>`, `team:<id>` or `role:<key>`
 * entries) with an optional `subject` and `message`. Output: `{ sent,
 * file_name }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "ReportSend".
 */
export interface ReportSend {
	dashboard_id: unknown;
	format?: unknown;
	message?: unknown;
	range?: unknown;
	subject?: unknown;
	to: unknown;
}
/**
 * A change's approval decision to record (`core.change.decide`): the
 * change ticket (in the run's organization or below, submitted for
 * approval), `decision` (`approved` or `rejected`, or a boolean), `by`
 * (who decided, e.g. the approval's `by`) and a `comment`. Raises
 * `change.approved` or `change.rejected`. Output: the change.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "ChangeDecide".
 */
export interface ChangeDecide {
	by?: unknown;
	comment?: unknown;
	decision: unknown;
	ticket_id: unknown;
}
/**
 * A project scope change's decision to record
 * (`core.project.scope.decide`): `scope_change_id` (submitted, on a
 * project in the run's organization or below), `decision` (`approved` or
 * `rejected`, or a boolean), `by` and a `comment`. An approved change's
 * hours are added to the budget. Raises `project.scope_change_approved`
 * or `project.scope_change_rejected`. Output: the scope change.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "ScopeDecide".
 */
export interface ScopeDecide {
	by?: unknown;
	comment?: unknown;
	decision: unknown;
	scope_change_id: unknown;
}
/**
 * A quote review's decision to record (`core.quote.review`): `review_id`
 * (from `quote.submitted_for_review`), `decision` (`approved`,
 * `changes_requested` or `rejected`, or a boolean), `by` and a `comment`.
 * Only the quote's open review counts: if the quote changed since, it
 * needs a new review and the step fails with `kind: conflict`. Raises
 * `quote.approved`, `quote.changes_requested` or `quote.rejected`, and
 * tells the quote's owner. Output: the quote's review state.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "QuoteReview".
 */
export interface QuoteReview {
	by?: unknown;
	comment?: unknown;
	decision: unknown;
	review_id: unknown;
}
/**
 * An email to log (`core.sales.log_email`): `email_id`, from a mailbox's
 * `email.received`. It's filed as an activity on each contact it's from or
 * to (on their client's latest open opportunity, if any) and on open leads
 * with those addresses. Output: `{ logged, unmatched: [address] }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "SalesLogEmail".
 */
export interface SalesLogEmail {
	email_id: unknown;
}
/**
 * Whose calendars to sync (`core.calendar.sync`): everyone set up (from
 * Microsoft 365 or an iCalendar feed), or `user_id`. Busy blocks are
 * replaced from a day back to 60 days ahead. Output: `{ people, blocks,
 * failed: [{ user_id, error }] }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "CalendarSync".
 */
export interface CalendarSync {
	user_id?: unknown;
}
/**
 * An appointment to write to its person's calendar
 * (`core.calendar.write`): `appointment_id`. When writing back is on (and
 * their calendar is Microsoft 365) the event is created, kept in step when
 * the appointment moves or changes hands, and removed when it's cancelled.
 * Output: `{ written, action, event_id }` or `{ written: false, reason }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "CalendarWrite".
 */
export interface CalendarWrite {
	appointment_id: unknown;
}
/**
 * What to gather for status reports (`core.project.status`): one
 * `project_id`, or every active project in the run's organization and
 * below; over the last `days` (default 7). Output: `{ projects: [{
 * project_id, name, client, status, period_start, period_end, health,
 * health_reasons, budget_hours, used_hours, burn_percent,
 * hours_in_period, tasks_done, tasks_total, completed, waiting, overdue,
 * milestones_reached, milestones_upcoming, scope_changes_pending,
 * manager_recipient }] }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "ProjectStatus".
 */
export interface ProjectStatus {
	days?: unknown;
	project_id?: unknown;
}
/**
 * A status report to save (`core.project.report`): `project_id` (in the
 * run's organization or below), `body` (Markdown), `client_visible`
 * (shown in the portal; default false), `period_start` and `period_end`
 * (`YYYY-MM-DD`), `author` (default "AI status reports"). Raises
 * `project.status_report`. Output: the report.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "ProjectReport".
 */
export interface ProjectReport {
	author?: unknown;
	body: unknown;
	client_visible?: unknown;
	period_end?: unknown;
	period_start?: unknown;
	project_id: unknown;
}
/**
 * Mapping rules to run (`core.asset.sync`): a dataset's rows become
 * assets, matched, updated and created by the rule. Give `mapping_id` for
 * one rule, or `dataset` (a dataset's key) for every rule on it that's on.
 * Run it after the step that fills the dataset. Output: `{ rows, created,
 * updated, unchanged, skipped, problems: [{ row, reason }] }`, summed over
 * the rules, plus `mappings` (how many ran) given `dataset`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "AssetSync".
 */
export interface AssetSync {
	dataset?: unknown;
	mapping_id?: unknown;
}
/**
 * A warranty to record (`core.asset.set_warranty`) for an asset at the
 * run's organization or below, as found by a lookup (`source`, e.g.
 * `dell`): added, or updated if the lookup recorded one with the same
 * `reference`. Dates are `YYYY-MM-DD`; `kind` is `manufacturer` (the
 * default), `extended`, `support` or `other`. Output: the warranty.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "AssetWarranty".
 */
export interface AssetWarranty {
	asset_id: unknown;
	ends_on: unknown;
	kind?: unknown;
	notes?: unknown;
	provider?: unknown;
	reference?: unknown;
	source: unknown;
	starts_on?: unknown;
}
/**
 * A public note to email to its ticket's requester
 * (`core.ticket.email_reply`), from the mailbox the ticket's mail came to
 * (else the nearest one at its organization or above). The subject carries
 * the ticket's `[HX#…]` token and the message continues the email thread,
 * so the requester's answer comes back to the ticket. Notes that aren't
 * for the requester (internal, written by them, already sent) are skipped.
 * Output: `{ sent: true, to, message_id }` or `{ sent: false, reason }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TicketEmailReply".
 */
export interface TicketEmailReply {
	/**
	 * The note's id (from `ticket.note_added`).
	 */
	note_id: unknown;
}
/**
 * A prompt (`ai.prompt`). Output: `{ text, model }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "AiPrompt".
 */
export interface AiPrompt {
	max_tokens?: unknown;
	/**
	 * Another model than the provider's (e.g. a faster one).
	 */
	model?: string | null;
	prompt: string;
	/**
	 * Instructions for the model.
	 */
	system?: string | null;
}
/**
 * Picking a label (`ai.classify`). Output: `{ label, model }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "AiClassify".
 */
export interface AiClassify {
	/**
	 * What the labels mean, or how to choose.
	 */
	instructions?: string | null;
	/**
	 * The labels to choose from (a list, or an expression giving one).
	 */
	labels: unknown;
	model?: string | null;
	/**
	 * What to classify.
	 */
	text: string;
}
/**
 * Pulling out data (`ai.extract`). Output: the data, as the schema says.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "AiExtract".
 */
export interface AiExtract {
	instructions?: string | null;
	model?: string | null;
	/**
	 * A JSON Schema (an object) of what to extract.
	 */
	schema: unknown;
	/**
	 * What to extract from.
	 */
	text: string;
}
/**
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "Settings".
 */
export interface Settings {
	/**
	 * At most this many runs of this workflow at once per organization.
	 */
	concurrency?: number | null;
	/**
	 * Fail the run if it takes longer than this, e.g. `1h`.
	 */
	timeout?: string | null;
}
/**
 * Which organization (`core.company.get`): `org_id`, default the run's.
 * Output: `{ company, sites }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "CompanyRef".
 */
export interface CompanyRef1 {
	org_id?: string | null;
}
/**
 * Client health for today (`core.health.compute`): each client company in
 * the run's organization and below gets a score, 0 to 100, from weighted
 * factors (satisfaction, NPS, SLA misses, reopened tickets, ticket volume,
 * renewals, sentiment), with the reasons; those that dropped 10 points or
 * more from a week before emit `client.health_declining`, and those whose
 * messages turned negative emit `client.sentiment_declining` (at most
 * weekly). It takes nothing. Output: `{ companies, declining: [{ org_id,
 * name, score, previous, reasons }], sentiment_falling: [{ org_id, name,
 * recent, previous, explanations }] }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "HealthCompute".
 */
export interface HealthCompute1 {}
/**
 * Articles to embed (`core.kb.embed`): published knowledge base articles
 * at the run's organization and below whose embedding is missing or stale
 * (their published title or text changed, they moved organization, or the
 * embeddings provider's model changed), up to `limit` (default 200), by
 * each organization's embeddings provider. Does nothing when pgvector
 * isn't installed. A provider that fails is reported without failing the
 * step. Output: `{ available, embedded, errors: [{ org_id, error }] }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "KbEmbed".
 */
export interface KbEmbed1 {
	limit?: unknown;
}
/**
 * Mail to fetch (`core.mail.fetch`): new messages from the enabled IMAP
 * and Microsoft 365 mailboxes at the run's organization and below, or just `mailbox_id`, up
 * to `limit` each (default 50). Each new message is kept and announced with
 * `email.received`. A mailbox that can't be reached is reported (and shown
 * on it) without failing the step. Output: `{ mailboxes: [{ mailbox_id,
 * address, fetched, new } | { mailbox_id, address, error }] }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "MailFetch".
 */
export interface MailFetch1 {
	limit?: unknown;
	mailbox_id?: unknown;
}
/**
 * A net promoter round (`core.nps.send`): contacts in the run's
 * organization and below who raised a ticket lately and weren't asked
 * lately (each company's Customer experience settings; companies can opt
 * out) are asked how likely they are to recommend you, 0 to 10, by email
 * when email and the portal's address are set up, and in the portal.
 * `campaign` names the round (default: the month, e.g. `2026-11`); `limit`
 * caps how many are asked (default 500). Output: `{ asked, emailed,
 * campaign }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "NpsSend".
 */
export interface NpsSend1 {
	campaign?: unknown;
	limit?: unknown;
}
/**
 * Tickets to embed (`core.ticket.embed`): those at the run's organization
 * and below whose embedding is missing or stale, newest changes first, up
 * to `limit` (default 200), by each organization's embeddings provider.
 * Does nothing when pgvector isn't installed (search by meaning is off). A
 * provider that fails is reported without failing the step. Output:
 * `{ available, embedded, errors: [{ org_id, error }] }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TicketEmbed".
 */
export interface TicketEmbed1 {
	limit?: unknown;
}
/**
 * What a ticket can be triaged to (`core.triage.options`): the queues at
 * `ticket_id`'s organization or above (or the run's organization's,
 * without one), and the priorities. Output: `{ queues: [{ id, name,
 * description }], queue_names (the names and "(none)"), priority_keys,
 * skill_names (the workforce skill catalog's and "(none)"), guide (a line
 * per queue, priority and skill, for a prompt) }`.
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "TriageOptions".
 */
export interface TriageOptions1 {
	ticket_id?: unknown;
}
/**
 * Skill gaps now (`core.workforce.insights`): who meets each queue's and
 * ticket type's requirements, recent tickets worked without them, and
 * certifications expiring within `days` (default 60) with the work only
 * their holder is qualified for. Output: `{ coverage, unqualified,
 * expiring, summary }` (`summary`: a line per finding, most serious
 * first).
 *
 * This interface was referenced by `HuxleyWorkflowFormatV1`'s JSON-Schema
 * via the `definition` "WorkforceInsights".
 */
export interface WorkforceInsights1 {
	days?: unknown;
}
