<script lang="ts">
import { onDestroy } from "svelte";
import { useQueryClient } from "@tanstack/svelte-query";
import { print } from "graphql";
import {
    AddPublicKeyDocument,
    AuthChallengeDocument,
    RemovePublicKeyDocument,
    UpdateFullnameDocument,
    UserSettingsDocument,
} from "$lib/graphql/generated";
import type { AuthChallengeQuery } from "$lib/graphql/generated";
import { query } from "$lib/graphql/client";
import Spinner from "$lib/components/Spinner.svelte";
import { graphqlMutation, graphqlQuery } from "$lib/graphql/query";
import { createPasskeyRegistration } from "$lib/webauthn";
import SshSignatureInput from "$lib/components/SshSignatureInput.svelte";

const queryClient = useQueryClient();

const settings = graphqlQuery(() => ({
    document: UserSettingsDocument,
}));

let fullname = $state("");
let fullnameLoaded = $state(false);
let fullnameSaving = $state(false);
let fullnameSuccess = $state(false);
let fullnameError = $state<string | null>(null);

let addKeyMode = $state<"closed" | "choose" | "ssh">("closed");
let authChallenge = $state<AuthChallengeQuery["authChallenge"] | null>(null);
let addKeyLoading = $state(false);
let addKeyError = $state<string | null>(null);
let removeKeyError = $state<string | null>(null);
let refreshInterval: ReturnType<typeof setInterval> | null = null;

$effect(() => {
    if (settings.data && !fullnameLoaded) {
        fullname = settings.data.me.fullname ?? "";
        fullnameLoaded = true;
    }
});

onDestroy(() => {
    stopChallengeRefresh();
});

const updateFullname = graphqlMutation(UpdateFullnameDocument, {
    onSuccess: () => {
        fullnameSaving = false;
        fullnameSuccess = true;
        fullnameError = null;
        setTimeout(() => (fullnameSuccess = false), 2000);
        queryClient.invalidateQueries({
            queryKey: [print(UserSettingsDocument)],
        });
    },
    onError: (e) => {
        fullnameSaving = false;
        fullnameError = e.message;
    },
});

const addPublicKey = graphqlMutation(AddPublicKeyDocument, {
    onSuccess: () => {
        addKeyMode = "closed";
        authChallenge = null;
        addKeyError = null;
        stopChallengeRefresh();
        queryClient.invalidateQueries({
            queryKey: [print(UserSettingsDocument)],
        });
    },
    onError: (e) => {
        addKeyError = e.message;
    },
});

const removePublicKey = graphqlMutation(RemovePublicKeyDocument, {
    onSuccess: () => {
        removeKeyError = null;
        queryClient.invalidateQueries({
            queryKey: [print(UserSettingsDocument)],
        });
    },
    onError: (e) => {
        removeKeyError = e.message;
    },
});

function saveFullname() {
    fullnameSaving = true;
    fullnameSuccess = false;
    fullnameError = null;
    updateFullname.mutate({ fullname: fullname.trim() });
}

async function startAddKey() {
    if (!settings.data) return;
    addKeyError = null;
    addKeyLoading = true;
    try {
        const data = await query(AuthChallengeDocument, {
            username: settings.data.me.username,
        });
        authChallenge = data.authChallenge;
        addKeyMode = "choose";
        startChallengeRefresh();
    } catch (e) {
        addKeyError = e instanceof Error ? e.message : "Failed to fetch challenge";
    } finally {
        addKeyLoading = false;
    }
}

async function refreshChallenge() {
    if (!settings.data) return;
    try {
        const data = await query(AuthChallengeDocument, {
            username: settings.data.me.username,
        });
        authChallenge = data.authChallenge;
    } catch {
        // Silently ignore
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

function cancelAddKey() {
    addKeyMode = "closed";
    authChallenge = null;
    addKeyError = null;
    stopChallengeRefresh();
}

async function addPasskey() {
    if (!authChallenge?.passkeyRegistration) return;
    addKeyError = null;
    addKeyLoading = true;
    try {
        const proof = await createPasskeyRegistration(authChallenge.passkeyRegistration);
        addPublicKey.mutate({ proof: JSON.stringify(proof) });
    } catch (e) {
        addKeyError = e instanceof Error ? e.message : "Passkey registration failed";
    } finally {
        addKeyLoading = false;
    }
}

function addSshKey(signature: string) {
    addKeyError = null;
    addPublicKey.mutate({ proof: signature });
}

function removeKey(fingerprint: string) {
    removeKeyError = null;
    removePublicKey.mutate({ fingerprint });
}
</script>

<svelte:head>
    <title>Settings - Skyr</title>
</svelte:head>

<div class="p-6 max-w-2xl mx-auto">
    <h1 class="font-bold text-gray-900 mb-8">Settings</h1>

    {#if settings.isPending}
        <Spinner />
    {:else if settings.error}
        <div
            class="p-4 bg-red-50 border border-red-200 rounded text-red-600"
        >
            {settings.error.message}
        </div>
    {:else}
        <!-- Profile section -->
        <section class="mb-10">
            <h2 class="font-medium text-gray-900 mb-4">Profile</h2>
            <div class="bg-white border border-gray-200 rounded-lg p-5">
                <div class="mb-4">
                    <label
                        class="block font-medium text-gray-500 mb-1"
                        for="username"
                    >
                        Username
                    </label>
                    <p id="username" class="text-gray-900">
                        {settings.data.me.username}
                    </p>
                </div>

                <div class="mb-4">
                    <label
                        class="block font-medium text-gray-500 mb-1"
                        for="email"
                    >
                        Email
                    </label>
                    <p id="email" class="text-gray-900">
                        {settings.data.me.email}
                    </p>
                </div>

                <form
                    onsubmit={(e) => {
                        e.preventDefault();
                        saveFullname();
                    }}
                >
                    <label
                        class="block font-medium text-gray-500 mb-1"
                        for="fullname"
                    >
                        Full name
                    </label>
                    <div class="flex gap-3">
                        <input
                            id="fullname"
                            type="text"
                            bind:value={fullname}
                            placeholder="Your full name"
                            class="flex-1 px-3 py-2 bg-gray-100 border border-gray-300 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-orange-500"
                        />
                        <button
                            type="submit"
                            disabled={fullnameSaving}
                            class="px-4 py-2 bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                        >
                            {fullnameSaving ? "Saving..." : "Save"}
                        </button>
                    </div>
                    {#if fullnameSuccess}
                        <p class="mt-2 text-green-700">Saved.</p>
                    {/if}
                    {#if fullnameError}
                        <p class="mt-2 text-red-600">
                            {fullnameError}
                        </p>
                    {/if}
                </form>
            </div>
        </section>

        <!-- Authentication keys section -->
        <section>
            <h2 class="font-medium text-gray-900 mb-4">
                Authentication Keys
            </h2>
            <div class="bg-white border border-gray-200 rounded-lg p-5">
                {#if settings.data.me.publicKeys.length === 0}
                    <p class="text-gray-500 mb-4">No keys registered.</p>
                {:else}
                    <ul class="space-y-3 mb-4">
                        {#each settings.data.me.publicKeys as fingerprint}
                            <li
                                class="flex items-center justify-between bg-gray-100 border border-gray-300 rounded px-3 py-2"
                            >
                                <code class="text-gray-600 truncate">
                                    {fingerprint}
                                </code>
                                <button
                                    onclick={() => removeKey(fingerprint)}
                                    disabled={removePublicKey.isPending}
                                    class="ml-3 text-red-600 hover:text-red-500 transition-colors disabled:opacity-50 shrink-0"
                                >
                                    Remove
                                </button>
                            </li>
                        {/each}
                    </ul>
                {/if}

                {#if removeKeyError}
                    <p class="mb-3 text-red-600">{removeKeyError}</p>
                {/if}

                {#if addKeyMode === "closed"}
                    <button
                        onclick={startAddKey}
                        disabled={addKeyLoading}
                        class="px-4 py-2 bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                    >
                        {addKeyLoading ? "Loading..." : "Add key"}
                    </button>
                {:else if addKeyMode === "choose"}
                    <div class="space-y-3">
                        {#if addKeyError}
                            <p class="text-red-600">{addKeyError}</p>
                        {/if}

                        <button
                            onclick={addPasskey}
                            disabled={addKeyLoading || addPublicKey.isPending}
                            class="w-full px-4 py-2 bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                        >
                            {addKeyLoading || addPublicKey.isPending
                                ? "Adding..."
                                : "Add passkey"}
                        </button>

                        <button
                            onclick={() => (addKeyMode = "ssh")}
                            class="w-full px-4 py-2 bg-gray-200 hover:bg-gray-300 text-gray-700 rounded font-medium transition-colors"
                        >
                            Add SSH key
                        </button>

                        <button
                            onclick={cancelAddKey}
                            class="w-full px-4 py-2 text-gray-500 hover:text-gray-700 transition-colors"
                        >
                            Cancel
                        </button>
                    </div>
                {:else if addKeyMode === "ssh"}
                    <div class="space-y-3">
                        {#if addKeyError}
                            <p class="text-red-600">{addKeyError}</p>
                        {/if}

                        <SshSignatureInput
                            challenge={authChallenge?.challenge ?? ""}
                            onsubmit={addSshKey}
                            submitLabel="Add SSH key"
                            pendingLabel="Adding..."
                            pending={addPublicKey.isPending}
                        />

                        <button
                            onclick={cancelAddKey}
                            class="w-full px-4 py-2 text-gray-500 hover:text-gray-700 transition-colors"
                        >
                            Cancel
                        </button>
                    </div>
                {/if}
            </div>
        </section>
    {/if}
</div>
