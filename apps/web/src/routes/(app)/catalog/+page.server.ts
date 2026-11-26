import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

type ProductInput = components['schemas']['CatalogProductInput'];

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [products, orgs] = await Promise.all([
		client.GET('/api/v1/catalog/products'),
		client.GET('/api/v1/orgs')
	]);
	if (products.response.status === 404) error(404, 'The catalog is turned off.');
	if (!products.data)
		error(products.response.status === 403 ? 403 : 503, 'Could not load the catalog.');
	return { products: products.data, orgs: orgs.data ?? [] };
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { errors: fieldErrors(problem), message: problemMessage(problem, fallback) });

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		let product: ProductInput;
		try {
			product = JSON.parse(formString(form, 'product')) as ProductInput;
		} catch {
			return fail(400, { errors: {}, message: 'The product could not be read.' });
		}
		const { error: problem, response } = id
			? await api(event).PUT('/api/v1/catalog/products/{id}', {
					params: { path: { id } },
					body: product
				})
			: await api(event).POST('/api/v1/catalog/products', {
					body: { orgId: formString(form, 'orgId'), product }
				});
		if (response.status !== 200 && response.status !== 201)
			return failed(response.status, problem, 'Could not save the product.');
		return { saved: id ? 'Product saved.' : 'Product added.' };
	},

	delete: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).DELETE('/api/v1/catalog/products/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (response.status !== 204) return failed(response.status, problem, 'Could not delete it.');
		return { saved: 'Product deleted.' };
	}
};
