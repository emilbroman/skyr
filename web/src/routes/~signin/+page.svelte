<script lang="ts">
	import { goto } from '$app/navigation';
	import { graphqlMutation } from '$lib/graphql/query';
	import { query } from '$lib/graphql/client';
	import { AuthChallengeDocument, SignInDocument } from '$lib/graphql/generated';
	import { setAuth } from '$lib/stores/auth';
	import { onDestroy } from 'svelte';

	let username = $state('');
	let challenge = $state<string | null>(null);
	let response = $state('');
	let error = $state<string | null>(null);
	let loading = $state(false);
	let step = $state<'username' | 'sign'>('username');
	let refreshInterval: ReturnType<typeof setInterval> | null = null;

	const signIn = graphqlMutation(SignInDocument, {
		onSuccess: (data) => {
			setAuth(data.signin.token, data.signin.user);
			goto('/');
		},
		onError: (e) => {
			error = e.message;
		}
	});

	async function fetchChallenge() {
		if (!username.trim()) return;
		error = null;
		loading = true;
		try {
			const data = await query(AuthChallengeDocument, { username: username.trim() });
			challenge = data.authChallenge;
			step = 'sign';
			startChallengeRefresh();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to fetch challenge';
		} finally {
			loading = false;
		}
	}

	async function refreshChallenge() {
		if (!username.trim()) return;
		try {
			const data = await query(AuthChallengeDocument, { username: username.trim() });
			challenge = data.authChallenge;
		} catch {
			// Silently ignore refresh errors — the user can still try with the current challenge
		}
	}

	function startChallengeRefresh() {
		stopChallengeRefresh();
		refreshInterval = setInterval(refreshChallenge, 60_000);
	}

	function stopChallengeRefresh() {
		if (refreshInterval) {
			clearInterval(refreshInterval);
			refreshInterval = null;
		}
	}

	onDestroy(stopChallengeRefresh);

	function parseResponse(raw: string): { pubkey: string; signature: string } | null {
		const marker = '-----BEGIN SSH SIGNATURE-----';
		const idx = raw.indexOf(marker);
		if (idx === -1) return null;
		const pubkey = raw.slice(0, idx).trim();
		const signature = raw.slice(idx).trim();
		if (!pubkey || !signature) return null;
		return { pubkey, signature };
	}

	function submitSignIn() {
		const parsed = parseResponse(response);
		if (!parsed) {
			error = 'Could not find both a public key and an SSH signature in the output. Make sure you pasted the full output.';
			return;
		}
		error = null;
		signIn.mutate({
			username: username.trim(),
			signature: parsed.signature,
			pubkey: parsed.pubkey
		});
	}

	function copyToClipboard(text: string) {
		navigator.clipboard.writeText(text);
	}

</script>

<div class="min-h-screen bg-gray-950 flex items-center justify-center p-4">
	<div class="w-full max-w-lg">
		<div class="text-center mb-8">
			<h1 class="text-3xl font-bold text-white tracking-tight">Skyr</h1>
			<p class="text-gray-400 mt-2">Sign in with your SSH key</p>
		</div>

		<div class="bg-gray-900 rounded-lg border border-gray-800 p-6">
			{#if error}
				<div class="mb-4 p-3 bg-red-900/30 border border-red-800 rounded text-red-300 text-sm">
					{error}
				</div>
			{/if}

			{#if step === 'username'}
				<form onsubmit={(e) => { e.preventDefault(); fetchChallenge(); }}>
					<label class="block mb-2 text-sm font-medium text-gray-300" for="username">
						Username
					</label>
					<input
						id="username"
						type="text"
						bind:value={username}
						placeholder="Enter your username"
						class="w-full px-3 py-2 bg-gray-800 border border-gray-700 rounded text-white placeholder-gray-500 focus:outline-none focus:border-indigo-500"
						disabled={loading}
					/>
					<button
						type="submit"
						class="w-full mt-4 px-4 py-2 bg-indigo-600 hover:bg-indigo-500 text-white rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
						disabled={loading || !username.trim()}
					>
						{loading ? 'Loading...' : 'Continue'}
					</button>
				</form>
			{:else}
				<div class="space-y-5">
					<div>
						<p class="text-sm text-gray-400 mb-3">
							Signing in as <span class="text-white font-medium">{username}</span>
							<button
								class="text-indigo-400 hover:text-indigo-300 ml-2 text-xs"
								onclick={() => { step = 'username'; challenge = null; error = null; stopChallengeRefresh(); }}
							>
								Change
							</button>
						</p>
					</div>

					<div>
						<p class="text-sm text-gray-300 mb-2">
							Run this command in your terminal:
						</p>
						<div class="relative">
							<pre class="bg-gray-800 border border-gray-700 rounded p-3 text-sm text-green-400 overflow-x-auto whitespace-pre-wrap break-all">cat ~/.ssh/id_ed25519.pub; echo -n '{challenge}' | ssh-keygen -Y sign -f ~/.ssh/id_ed25519 -n skyr-auth-challenge</pre>
							<button
								class="absolute top-2 right-2 text-gray-400 hover:text-white text-xs px-2 py-1 bg-gray-700 rounded"
								onclick={() => copyToClipboard(`cat ~/.ssh/id_ed25519.pub; echo -n '${challenge}' | ssh-keygen -Y sign -f ~/.ssh/id_ed25519 -n skyr-auth-challenge`)}
							>
								Copy
							</button>
						</div>
						<label class="block mt-2 text-sm text-gray-400" for="response">
							Paste the full output:
						</label>
						<textarea
							id="response"
							bind:value={response}
							placeholder="ssh-ed25519 AAAA...&#10;-----BEGIN SSH SIGNATURE-----&#10;...&#10;-----END SSH SIGNATURE-----"
							rows={8}
							class="w-full mt-1 px-3 py-2 bg-gray-800 border border-gray-700 rounded text-white placeholder-gray-500 focus:outline-none focus:border-indigo-500 font-mono text-xs"
						></textarea>
					</div>

					<div class="pt-1 flex items-center gap-2 text-xs text-gray-500">
						<svg class="w-4 h-4 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
						</svg>
						<span>The challenge auto-refreshes every minute. Make sure to sign the latest command shown above.</span>
					</div>

					<button
						onclick={submitSignIn}
						class="w-full px-4 py-2 bg-indigo-600 hover:bg-indigo-500 text-white rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
						disabled={signIn.isPending || !response.trim()}
					>
						{signIn.isPending ? 'Signing in...' : 'Sign In'}
					</button>
				</div>
			{/if}
		</div>
	</div>
</div>
