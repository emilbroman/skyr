<script lang="ts">
	import { goto } from '$app/navigation';
	import { query, mutate } from '$lib/graphql/client';
	import { AuthChallengeDocument, SignInDocument } from '$lib/graphql/generated';
	import { setAuth } from '$lib/stores/auth';

	let username = $state('');
	let challenge = $state<string | null>(null);
	let pubkey = $state('');
	let signature = $state('');
	let error = $state<string | null>(null);
	let loading = $state(false);
	let step = $state<'username' | 'sign'>('username');

	async function fetchChallenge() {
		if (!username.trim()) return;
		error = null;
		loading = true;
		try {
			const data = await query(AuthChallengeDocument, { username: username.trim() });
			challenge = data.authChallenge;
			step = 'sign';
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to fetch challenge';
		} finally {
			loading = false;
		}
	}

	async function submitSignIn() {
		if (!signature.trim() || !pubkey.trim()) {
			error = 'Both public key and signature are required';
			return;
		}
		error = null;
		loading = true;
		try {
			const data = await mutate(SignInDocument, {
				username: username.trim(),
				signature: signature.trim(),
				pubkey: pubkey.trim()
			});
			setAuth(data.signin.token, data.signin.user);
			goto('/repos');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Sign-in failed';
		} finally {
			loading = false;
		}
	}

	function copyToClipboard(text: string) {
		navigator.clipboard.writeText(text);
	}

	$effect(() => {
		// Reset to step 1 when username changes
		if (step === 'sign') {
			// Don't auto-reset while on sign step
		}
	});
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
								onclick={() => { step = 'username'; challenge = null; error = null; }}
							>
								Change
							</button>
						</p>
					</div>

					<div>
						<p class="text-sm text-gray-300 mb-2">
							1. Copy your public key:
						</p>
						<div class="relative">
							<pre class="bg-gray-800 border border-gray-700 rounded p-3 text-sm text-green-400 overflow-x-auto">cat ~/.ssh/id_ed25519.pub</pre>
							<button
								class="absolute top-2 right-2 text-gray-400 hover:text-white text-xs px-2 py-1 bg-gray-700 rounded"
								onclick={() => copyToClipboard('cat ~/.ssh/id_ed25519.pub')}
							>
								Copy
							</button>
						</div>
						<label class="block mt-2 text-sm text-gray-400" for="pubkey">
							Paste your public key:
						</label>
						<textarea
							id="pubkey"
							bind:value={pubkey}
							placeholder="ssh-ed25519 AAAA..."
							rows={2}
							class="w-full mt-1 px-3 py-2 bg-gray-800 border border-gray-700 rounded text-white placeholder-gray-500 focus:outline-none focus:border-indigo-500 font-mono text-xs"
						></textarea>
					</div>

					<div>
						<p class="text-sm text-gray-300 mb-2">
							2. Sign the challenge by running:
						</p>
						<div class="relative">
							<pre class="bg-gray-800 border border-gray-700 rounded p-3 text-sm text-green-400 overflow-x-auto whitespace-pre-wrap break-all">echo -n '{challenge}' | ssh-keygen -Y sign -f ~/.ssh/id_ed25519 -n skyr-auth-challenge</pre>
							<button
								class="absolute top-2 right-2 text-gray-400 hover:text-white text-xs px-2 py-1 bg-gray-700 rounded"
								onclick={() => copyToClipboard(`echo -n '${challenge}' | ssh-keygen -Y sign -f ~/.ssh/id_ed25519 -n skyr-auth-challenge`)}
							>
								Copy
							</button>
						</div>
						<label class="block mt-2 text-sm text-gray-400" for="signature">
							Paste the signature output:
						</label>
						<textarea
							id="signature"
							bind:value={signature}
							placeholder="-----BEGIN SSH SIGNATURE-----&#10;...&#10;-----END SSH SIGNATURE-----"
							rows={6}
							class="w-full mt-1 px-3 py-2 bg-gray-800 border border-gray-700 rounded text-white placeholder-gray-500 focus:outline-none focus:border-indigo-500 font-mono text-xs"
						></textarea>
					</div>

					<div class="pt-1 flex items-center gap-2 text-xs text-gray-500">
						<svg class="w-4 h-4 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
						</svg>
						<span>The challenge expires in ~2 minutes. If sign-in fails, click Continue again to get a fresh challenge.</span>
					</div>

					<button
						onclick={submitSignIn}
						class="w-full px-4 py-2 bg-indigo-600 hover:bg-indigo-500 text-white rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
						disabled={loading || !signature.trim() || !pubkey.trim()}
					>
						{loading ? 'Signing in...' : 'Sign In'}
					</button>
				</div>
			{/if}
		</div>
	</div>
</div>
