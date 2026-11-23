/** Cookie holding the organization the user is working in (the org switcher). */
export const ORG_COOKIE = 'hx_org';

export type OrgOption = { id: string; name: string; kind: string; parentName: string | null };
