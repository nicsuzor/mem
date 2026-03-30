import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
	plugins: [tailwindcss(), sveltekit()],
	server: {
		allowedHosts: ['dev3.stoat-musical.ts.net']
	},
	test: {
		include: ['src/**/*.test.ts'],
	}
});
