<script lang="ts">
	import { goto } from '$app/navigation';
	import { isAuthenticated } from '$lib/stores/auth';
	import { onMount } from 'svelte';

	onMount(() => {
		return isAuthenticated.subscribe((authed) => {
			if (authed) {
				goto('/repos');
			} else {
				goto('/signin');
			}
		});
	});
</script>

<div class="min-h-screen bg-gray-950 flex items-center justify-center">
	<p class="text-gray-400">Redirecting...</p>
</div>
