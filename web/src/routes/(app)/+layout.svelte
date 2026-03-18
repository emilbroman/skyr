<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { isAuthenticated, user, clearAuth } from '$lib/stores/auth';
	import TokenRefresh from '$lib/components/TokenRefresh.svelte';
	import { onMount } from 'svelte';

	let { children } = $props();

	onMount(() => {
		return isAuthenticated.subscribe((authed) => {
			if (!authed) {
				goto('/signin');
			}
		});
	});

	function signOut() {
		clearAuth();
		goto('/signin');
	}
</script>

{#if $isAuthenticated}
	<div class="min-h-screen bg-gray-950 flex flex-col">
		<TokenRefresh />

		<div class="flex flex-1">
			<!-- Sidebar -->
			<nav class="w-56 bg-gray-900 border-r border-gray-800 flex flex-col shrink-0">
				<div class="p-4 border-b border-gray-800">
					<a href="/repos" class="text-xl font-bold text-white tracking-tight">Skyr</a>
				</div>

				<div class="flex-1 p-3">
					<a
						href="/repos"
						class="block px-3 py-2 rounded text-sm transition-colors {$page.url.pathname.startsWith('/repos') ? 'bg-gray-800 text-white' : 'text-gray-400 hover:text-gray-200 hover:bg-gray-800/50'}"
					>
						Repositories
					</a>
				</div>

				<div class="p-3 border-t border-gray-800">
					<div class="px-3 py-2 text-sm text-gray-400">
						{$user?.username ?? ''}
					</div>
					<button
						onclick={signOut}
						class="w-full text-left px-3 py-2 rounded text-sm text-gray-400 hover:text-gray-200 hover:bg-gray-800/50 transition-colors"
					>
						Sign Out
					</button>
				</div>
			</nav>

			<!-- Main content -->
			<main class="flex-1 min-w-0">
				{@render children()}
			</main>
		</div>
	</div>
{/if}
