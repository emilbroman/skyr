<script lang="ts">
import { onDestroy, onMount } from "svelte";
import { goto } from "$app/navigation";
import { page } from "$app/stores";
import { query } from "$lib/graphql/client";
import { AuthChallengeDocument, SignInDocument } from "$lib/graphql/generated";
import type { AuthChallengeQuery } from "$lib/graphql/generated";
import { graphqlMutation } from "$lib/graphql/query";
import { setAuth } from "$lib/stores/auth";
import { createPasskeyAssertion } from "$lib/webauthn";
import SshSignatureInput from "$lib/components/SshSignatureInput.svelte";
import SkyrLogo from "$lib/components/SkyrLogo.svelte";

let username = $state("");
let authChallenge = $state<AuthChallengeQuery["authChallenge"] | null>(null);
let error = $state<string | null>(null);
let loading = $state(false);
let step = $state<"username" | "sign">("username");
let showSshFlow = $state(false);
let refreshInterval: ReturnType<typeof setInterval> | null = null;

onMount(() => {
    const params = new URL(window.location.href).searchParams;
    const u = params.get("username");
    if (u) username = u;
});

const signIn = graphqlMutation(SignInDocument, {
    onSuccess: (data) => {
        setAuth(data.signin.token, data.signin.user);
        goto("/");
    },
    onError: (e) => {
        error = e.message;
    },
});

async function fetchChallenge() {
    if (!username.trim()) return;
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

function submitSshSignIn(signature: string) {
    error = null;
    signIn.mutate({
        username: username.trim(),
        proof: signature,
    });
}

async function submitPasskeySignIn() {
    if (!authChallenge?.passkeySignin) return;
    error = null;
    loading = true;
    try {
        const proof = await createPasskeyAssertion(authChallenge.passkeySignin);
        signIn.mutate({
            username: username.trim(),
            proof: JSON.stringify(proof),
        });
    } catch (e) {
        error = e instanceof Error ? e.message : "Passkey authentication failed";
    } finally {
        loading = false;
    }
}
</script>

<svelte:head>
    <title>Sign In – Skyr</title>
</svelte:head>

<div class="flex-1 flex items-center justify-center p-4">
  <div class="w-full max-w-sm">
    <div class="text-center mb-6">
      <p class="text-xs text-gray-500">Sign in to your account</p>
    </div>

    <div class="bg-white rounded border border-gray-200 p-5">
      {#if error}
        <div class="mb-3 p-2 bg-red-50 border border-red-200 rounded text-xs text-red-600">
          {error}
        </div>
      {/if}

      {#if step === "username"}
        <form
          onsubmit={(e) => {
            e.preventDefault();
            fetchChallenge();
          }}
        >
          <label class="block mb-1 text-xs font-medium text-gray-500" for="username">
            Username
          </label>
          <input
            id="username"
            type="text"
            bind:value={username}
            placeholder="Enter your username"
            class="w-full px-2.5 py-1.5 text-xs bg-white border border-gray-200 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-blue-500"
            disabled={loading}
          />
          <button
            type="submit"
            class="w-full mt-3 px-3 py-1.5 text-xs font-medium text-white bg-gray-900 rounded hover:bg-gray-800 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            disabled={loading || !username.trim()}
          >
            {loading ? "Loading..." : "Continue"}
          </button>
        </form>

        <p class="mt-4 text-center text-xs text-gray-500">
          Don't have an account?
          <a
            href="/~signup{username.trim() ? `?username=${encodeURIComponent(username.trim())}` : ''}"
            class="text-gray-700 hover:text-blue-600 underline-offset-2 hover:underline"
          >
            Sign up
          </a>
        </p>
      {:else}
        <div class="space-y-3">
          <div>
            <p class="text-xs text-gray-500">
              Signing in as <span class="text-gray-900 font-medium">{username}</span>
              <button
                class="text-xs text-gray-500 hover:text-blue-600 ml-2 underline-offset-2 hover:underline"
                onclick={() => {
                  step = "username";
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

          {#if authChallenge?.passkeySignin}
            <button
              onclick={submitPasskeySignIn}
              class="w-full px-3 py-1.5 text-xs font-medium text-white bg-gray-900 rounded hover:bg-gray-800 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
              disabled={loading || signIn.isPending}
            >
              {loading || signIn.isPending ? "Signing in..." : "Sign in with passkey"}
            </button>
          {/if}

          {#if !showSshFlow}
            <button
              onclick={() => (showSshFlow = true)}
              class="w-full px-3 py-1.5 text-xs font-medium text-gray-700 bg-white border border-gray-200 rounded hover:border-gray-300 hover:text-gray-900 transition-colors"
            >
              Sign in with SSH signature
            </button>
          {:else}
            <SshSignatureInput
              challenge={authChallenge?.challenge ?? ""}
              onsubmit={submitSshSignIn}
              submitLabel="Sign in with SSH signature"
              pendingLabel="Signing in..."
              pending={signIn.isPending}
              showRefreshNote
            />
          {/if}

          <p class="text-center text-xs text-gray-500">
            Don't have an account?
            <a
              href="/~signup?username={encodeURIComponent(username.trim())}"
              class="text-gray-700 hover:text-blue-600 underline-offset-2 hover:underline"
            >
              Sign up
            </a>
          </p>
        </div>
      {/if}
    </div>

    <SkyrLogo class="w-6 h-6 mx-auto mt-5" />
  </div>
</div>
