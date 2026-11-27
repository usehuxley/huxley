// Types for the Workbench shell (components/Workbench.svelte).

import type { Component } from 'svelte';

type Icon = Component<{ size?: number | string; class?: string }>;

export type SpaceMenuItem = {
	href: string;
	label: string;
	/** A count shown at the end, e.g. open tickets in a queue. */
	count?: number | null;
	/** Shows the count as urgent. */
	hot?: boolean;
};

export type SpaceMenuGroup = { label?: string; items: SpaceMenuItem[] };

/** A top-level area of the app: one button on the rail and its own menu. */
export type Space = {
	id: string;
	label: string;
	icon: Icon;
	/** Where the rail button goes. */
	href: string;
	/** Path prefixes that belong to this space, e.g. `/tickets`. `/` matches only the root. */
	paths: string[];
	badge?: number | null;
	menu?: SpaceMenuGroup[];
};
