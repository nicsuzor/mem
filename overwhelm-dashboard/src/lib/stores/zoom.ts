import { writable } from 'svelte/store';

/** Current zoom scale from ZoomContainer — used for progressive label reveal. */
export const zoomScale = writable(1.0);
