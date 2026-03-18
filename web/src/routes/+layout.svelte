<script lang="ts">
	import '../app.css';
	import { startExpiryWatch, stopExpiryWatch } from '$lib/stores/auth';
	import { onMount, onDestroy } from 'svelte';

	let { children } = $props();

	onMount(() => {
		startExpiryWatch();
	});

	onDestroy(() => {
		stopExpiryWatch();
	});
</script>

{@render children()}
