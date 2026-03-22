<script lang="ts">
import { onDestroy, onMount } from "svelte";
import { goto } from "$app/navigation";
import { query } from "$lib/graphql/client";
import { AuthChallengeDocument, SignupDocument } from "$lib/graphql/generated";
import type { AuthChallengeQuery } from "$lib/graphql/generated";
import { graphqlMutation } from "$lib/graphql/query";
import { setAuth } from "$lib/stores/auth";
import { createPasskeyRegistration } from "$lib/webauthn";

let username = $state("");
let email = $state("");
let fullname = $state("");
let authChallenge = $state<AuthChallengeQuery["authChallenge"] | null>(null);
let response = $state("");
let error = $state<string | null>(null);
let loading = $state(false);
let step = $state<"form" | "sign">("form");
let showSshFlow = $state(false);
let refreshInterval: ReturnType<typeof setInterval> | null = null;

onMount(() => {
    const params = new URL(window.location.href).searchParams;
    const u = params.get("username");
    if (u) username = u;
});

const signup = graphqlMutation(SignupDocument, {
    onSuccess: (data) => {
        setAuth(data.signup.token, data.signup.user);
        goto("/");
    },
    onError: (e) => {
        error = e.message;
    },
});

async function fetchChallenge() {
    if (!username.trim() || !email.trim()) return;
    error = null;
    loading = true;
    try {
        const data = await query(AuthChallengeDocument, {
            username: username.trim(),
        });
        authChallenge = data.authChallenge;
        step = "sign";
        showSshFlow = false;
        startChallengeRefresh();
    } catch (e) {
        error = e instanceof Error ? e.message : "Failed to fetch challenge";
    } finally {
        loading = false;
    }
}

async function refreshChallenge() {
    if (!username.trim()) return;
    try {
        const data = await query(AuthChallengeDocument, {
            username: username.trim(),
        });
        authChallenge = data.authChallenge;
    } catch {
        // Silently ignore refresh errors
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

function parseSignature(raw: string): string | null {
    const trimmed = raw.trim();
    if (!trimmed.startsWith("-----BEGIN SSH SIGNATURE-----")) return null;
    if (!trimmed.endsWith("-----END SSH SIGNATURE-----")) return null;
    return trimmed;
}

function submitSshSignup() {
    const signature = parseSignature(response);
    if (!signature) {
        error = "Could not find a valid SSH signature. Make sure you pasted the full output.";
        return;
    }
    error = null;
    signup.mutate({
        username: username.trim(),
        email: email.trim(),
        proof: signature,
    });
}

async function submitPasskeySignup() {
    if (!authChallenge?.passkeyRegistration) return;
    error = null;
    loading = true;
    try {
        const proof = await createPasskeyRegistration(authChallenge.passkeyRegistration);
        signup.mutate({
            username: username.trim(),
            email: email.trim(),
            proof: JSON.stringify(proof),
        });
    } catch (e) {
        error = e instanceof Error ? e.message : "Passkey registration failed";
    } finally {
        loading = false;
    }
}

function copyToClipboard(text: string) {
    navigator.clipboard.writeText(text);
}
</script>

<div class="min-h-screen bg-gray-100 flex items-center justify-center p-4">
  <div class="w-full max-w-lg">
    <div class="text-center mb-8">
      <h1 class="text-xl font-bold text-gray-900">Skyr</h1>
      <p class="text-gray-500 mt-2">Create your account</p>
    </div>

    <div class="bg-white rounded-lg border border-gray-200 p-6">
      {#if error}
        <div
          class="mb-4 p-3 bg-red-50 border border-red-200 rounded text-red-600"
        >
          {error}
        </div>
      {/if}

      {#if step === "form"}
        <form
          onsubmit={(e) => {
            e.preventDefault();
            fetchChallenge();
          }}
        >
          <label
            class="block mb-2 font-medium text-gray-600"
            for="username"
          >
            Username
          </label>
          <input
            id="username"
            type="text"
            bind:value={username}
            placeholder="Choose a username"
            class="w-full px-3 py-2 bg-gray-100 border border-gray-300 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-orange-500"
            disabled={loading}
          />

          <label
            class="block mt-4 mb-2 font-medium text-gray-600"
            for="email"
          >
            Email
          </label>
          <input
            id="email"
            type="email"
            bind:value={email}
            placeholder="you@example.com"
            class="w-full px-3 py-2 bg-gray-100 border border-gray-300 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-orange-500"
            disabled={loading}
          />

          <label
            class="block mt-4 mb-2 font-medium text-gray-600"
            for="fullname"
          >
            Full name <span class="text-gray-400 font-normal">(optional)</span>
          </label>
          <input
            id="fullname"
            type="text"
            bind:value={fullname}
            placeholder="Your full name"
            class="w-full px-3 py-2 bg-gray-100 border border-gray-300 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-orange-500"
            disabled={loading}
          />

          <button
            type="submit"
            class="w-full mt-4 px-4 py-2 bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            disabled={loading || !username.trim() || !email.trim()}
          >
            {loading ? "Loading..." : "Continue"}
          </button>
        </form>

        <p class="mt-4 text-center text-gray-500">
          Already have an account?
          <a
            href="/~signin{username.trim() ? `?username=${encodeURIComponent(username.trim())}` : ''}"
            class="text-orange-600 hover:text-orange-500"
          >
            Sign in
          </a>
        </p>
      {:else}
        <div class="space-y-5">
          <div>
            <p class="text-gray-500 mb-3">
              Signing up as <span class="text-gray-900 font-medium"
                >{username}</span
              >
              <button
                class="text-orange-600 hover:text-orange-500 ml-2"
                onclick={() => {
                  step = "form";
                  authChallenge = null;
                  error = null;
                  showSshFlow = false;
                  stopChallengeRefresh();
                }}
              >
                Change
              </button>
            </p>
          </div>

          <button
            onclick={submitPasskeySignup}
            class="w-full px-4 py-2 bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            disabled={loading || signup.isPending}
          >
            {loading || signup.isPending ? "Signing up..." : "Sign up with passkey"}
          </button>

          {#if !showSshFlow}
            <button
              onclick={() => (showSshFlow = true)}
              class="w-full px-4 py-2 bg-gray-200 hover:bg-gray-300 text-gray-700 rounded font-medium transition-colors"
            >
              Sign up with SSH signature
            </button>
          {:else}
            <div class="space-y-4">
              <div>
                <p class="text-gray-600 mb-2">
                  Run this command in your terminal:
                </p>
                <div class="relative">
                  <pre
                    class="bg-gray-100 border border-gray-300 rounded p-3 text-green-700 overflow-x-auto whitespace-pre-wrap break-all">echo -n '{authChallenge?.challenge}' | ssh-keygen -Y sign -f ~/.ssh/id_ed25519 -n skyr-auth-challenge</pre>
                  <button
                    class="absolute top-2 right-2 text-gray-500 hover:text-gray-900 px-2 py-1 bg-gray-200 rounded"
                    onclick={() =>
                      copyToClipboard(
                        `echo -n '${authChallenge?.challenge}' | ssh-keygen -Y sign -f ~/.ssh/id_ed25519 -n skyr-auth-challenge`,
                      )}
                  >
                    Copy
                  </button>
                </div>
                <label class="block mt-2 text-gray-500" for="response">
                  Paste the full output:
                </label>
                <textarea
                  id="response"
                  bind:value={response}
                  placeholder="-----BEGIN SSH SIGNATURE-----&#10;...&#10;-----END SSH SIGNATURE-----"
                  rows={8}
                  class="w-full mt-1 px-3 py-2 bg-gray-100 border border-gray-300 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-orange-500 font-mono text-xs"
                ></textarea>
              </div>

              <div class="pt-1 flex items-center gap-2 text-gray-400">
                <svg
                  class="w-4 h-4 shrink-0"
                  fill="none"
                  viewBox="0 0 24 24"
                  stroke="currentColor"
                >
                  <path
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    stroke-width="2"
                    d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
                  />
                </svg>
                <span
                  >The challenge auto-refreshes every minute. Make sure to sign the
                  latest command shown above.</span
                >
              </div>

              <button
                onclick={submitSshSignup}
                class="w-full px-4 py-2 bg-gray-200 hover:bg-gray-300 text-gray-700 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                disabled={signup.isPending || !response.trim()}
              >
                {signup.isPending ? "Signing up..." : "Sign up with SSH signature"}
              </button>
            </div>
          {/if}

          <p class="text-center text-gray-500">
            Already have an account?
            <a
              href="/~signin?username={encodeURIComponent(username.trim())}"
              class="text-orange-600 hover:text-orange-500"
            >
              Sign in
            </a>
          </p>
        </div>
      {/if}
    </div>
  </div>
</div>
