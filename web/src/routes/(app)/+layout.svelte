<script lang="ts">
	import { goto } from '$app/navigation';
	import { isAuthenticated, user, clearAuth } from '$lib/stores/auth';
	import { onMount } from 'svelte';

	let { children } = $props();

	onMount(() => {
		return isAuthenticated.subscribe((authed) => {
			if (!authed) {
				goto('/~signin');
			}
		});
	});

	function signOut() {
		clearAuth();
		goto('/~signin');
	}
</script>

{#if $isAuthenticated}
	<div class="min-h-screen bg-gray-950 flex flex-col">
		<!-- Header -->
		<header class="h-14 bg-gray-900 border-b border-gray-800 flex items-center justify-between px-4 shrink-0">
			<a href="/" class="text-xl font-bold text-white tracking-tight">Skyr</a>

			<div class="flex items-center gap-4">
				<span class="text-sm text-gray-400">{$user?.username ?? ''}</span>
				<button
					onclick={signOut}
					class="text-sm text-gray-400 hover:text-gray-200 transition-colors"
				>
					Sign Out
				</button>
			</div>
		</header>

		<!-- Main content -->
		<main class="flex-1 min-w-0">
			{@render children()}
		</main>
	</div>
{/if}
