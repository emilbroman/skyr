<script lang="ts">
import { onDestroy, onMount } from "svelte";
import { goto } from "$app/navigation";
import { query } from "$lib/graphql/client";
import { AuthChallengeDocument, SignupDocument } from "$lib/graphql/generated";
import type { AuthChallengeQuery } from "$lib/graphql/generated";
import { graphqlMutation } from "$lib/graphql/query";
import { setAuth } from "$lib/stores/auth";
import { createPasskeyRegistration } from "$lib/webauthn";
import SshSignatureInput from "$lib/components/SshSignatureInput.svelte";
import SkyrLogo from "$lib/components/SkyrLogo.svelte";

let username = $state("");
let email = $state("");
let fullname = $state("");
let authChallenge = $state<AuthChallengeQuery["authChallenge"] | null>(null);
let error = $state<string | null>(null);
let usernameError = $state<string | null>(null);
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
    usernameError = null;
    loading = true;
    try {
        const data = await query(AuthChallengeDocument, {
            username: username.trim(),
        });
        if (data.authChallenge.taken) {
            usernameError = "This username is already taken.";
            return;
        }
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

function submitSshSignup(signature: string) {
    error = null;
    signup.mutate({
        username: username.trim(),
        email: email.trim(),
        proof: signature,
        fullname: fullname.trim() || null,
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
            fullname: fullname.trim() || null,
        });
    } catch (e) {
        error = e instanceof Error ? e.message : "Passkey registration failed";
    } finally {
        loading = false;
    }
}
</script>

<svelte:head>
    <title>Sign Up - Skyr</title>
</svelte:head>

<div class="flex-1 flex items-center justify-center p-4">
  <div class="w-full max-w-lg">
    <div class="text-center mb-8">
      <p class="text-gray-500">Create your account</p>
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
            oninput={() => (usernameError = null)}
            placeholder="Choose a username"
            class="w-full px-3 py-2 bg-gray-100 border border-gray-300 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-orange-500"
            disabled={loading}
          />
          {#if usernameError}
            <p class="mt-1 text-sm text-red-600">{usernameError}</p>
          {/if}

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
            <SshSignatureInput
              challenge={authChallenge?.challenge ?? ""}
              onsubmit={submitSshSignup}
              submitLabel="Sign up with SSH signature"
              pendingLabel="Signing up..."
              pending={signup.isPending}
              showRefreshNote
            />
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

    <SkyrLogo class="w-8 h-8 mx-auto mt-6" />
  </div>
</div>
