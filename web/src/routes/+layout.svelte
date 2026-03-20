<script lang="ts">
	import '../app.css';
	import { QueryClientProvider, QueryClient } from '@tanstack/svelte-query';
	import { startExpiryWatch, stopExpiryWatch } from '$lib/stores/auth';
	import { onMount, onDestroy } from 'svelte';

	let { children } = $props();

	const queryClient = new QueryClient({
		defaultOptions: {
			queries: {
				staleTime: 30_000,
				refetchOnWindowFocus: false
			}
		}
	});

	onMount(() => {
		startExpiryWatch();
	});

	onDestroy(() => {
		stopExpiryWatch();
	});
</script>

<QueryClientProvider client={queryClient}>
	{@render children()}
</QueryClientProvider>
