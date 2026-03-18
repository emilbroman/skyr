<script lang="ts">
	import { isExpiringSoon, isAuthenticated, token, user, setAuth } from '$lib/stores/auth';
	import { query, mutate } from '$lib/graphql/client';
	import { AuthChallengeDocument, SignInDocument } from '$lib/graphql/generated';
	import { get } from 'svelte/store';

	let challenge = $state<string | null>(null);
	let pubkey = $state('');
	let signature = $state('');
	let error = $state<string | null>(null);
	let expanded = $state(false);
	let loading = $state(false);

	async function refreshChallenge() {
		const u = get(user);
		if (!u) return;
		try {
			const data = await query(AuthChallengeDocument, { username: u.username });
			challenge = data.authChallenge;
			expanded = true;
			error = null;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to fetch challenge';
		}
	}

	async function submitRefresh() {
		const u = get(user);
		if (!u || !signature.trim() || !pubkey.trim()) return;
		loading = true;
		error = null;
		try {
			const data = await mutate(SignInDocument, {
				username: u.username,
				signature: signature.trim(),
				pubkey: pubkey.trim()
			});
			setAuth(data.signin.token, data.signin.user);
			expanded = false;
			challenge = null;
			signature = '';
		} catch (e) {
			error = e instanceof Error ? e.message : 'Refresh failed';
		} finally {
			loading = false;
		}
	}

	function copyToClipboard(text: string) {
		navigator.clipboard.writeText(text);
	}
</script>

{#if $isExpiringSoon && $isAuthenticated}
	<div class="bg-amber-900/40 border-b border-amber-800 px-4 py-2">
		<div class="flex items-center justify-between">
			<p class="text-amber-300 text-sm">
				Your session expires soon.
			</p>
			{#if !expanded}
				<button
					class="text-amber-200 hover:text-white text-sm font-medium px-3 py-1 bg-amber-800/50 rounded"
					onclick={refreshChallenge}
				>
					Refresh Session
				</button>
			{/if}
		</div>
		{#if expanded && challenge}
			<div class="mt-3 space-y-3">
				{#if error}
					<p class="text-red-300 text-sm">{error}</p>
				{/if}
				<div>
					<p class="text-sm text-amber-200 mb-1">Paste your public key:</p>
					<textarea
						bind:value={pubkey}
						placeholder="ssh-ed25519 AAAA..."
						rows={2}
						class="w-full px-2 py-1 bg-gray-800 border border-gray-700 rounded text-white font-mono text-xs"
					></textarea>
				</div>
				<div>
					<p class="text-sm text-amber-200 mb-1">Run this command and paste the output:</p>
					<div class="relative">
						<pre class="bg-gray-800 border border-gray-700 rounded p-2 text-xs text-green-400 overflow-x-auto">echo -n '{challenge}' | ssh-keygen -Y sign -f ~/.ssh/id_ed25519 -n skyr-auth-challenge</pre>
						<button
							class="absolute top-1 right-1 text-gray-400 hover:text-white text-xs px-1.5 py-0.5 bg-gray-700 rounded"
							onclick={() => copyToClipboard(`echo -n '${challenge}' | ssh-keygen -Y sign -f ~/.ssh/id_ed25519 -n skyr-auth-challenge`)}
						>
							Copy
						</button>
					</div>
					<textarea
						bind:value={signature}
						placeholder="-----BEGIN SSH SIGNATURE-----"
						rows={4}
						class="w-full mt-1 px-2 py-1 bg-gray-800 border border-gray-700 rounded text-white font-mono text-xs"
					></textarea>
				</div>
				<div class="flex gap-2">
					<button
						onclick={submitRefresh}
						class="px-3 py-1 bg-indigo-600 hover:bg-indigo-500 text-white rounded text-sm disabled:opacity-50"
						disabled={loading || !signature.trim() || !pubkey.trim()}
					>
						{loading ? 'Refreshing...' : 'Submit'}
					</button>
					<button
						onclick={() => { expanded = false; }}
						class="px-3 py-1 bg-gray-700 hover:bg-gray-600 text-gray-300 rounded text-sm"
					>
						Dismiss
					</button>
				</div>
			</div>
		{/if}
	</div>
{/if}
