import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

export default defineConfig({
	resolve: {
		// Generated code imports the SDK by name, as users' code does.
		alias: { '@usehuxley/sdk': fileURLToPath(new URL('./src/index.ts', import.meta.url)) }
	}
});
