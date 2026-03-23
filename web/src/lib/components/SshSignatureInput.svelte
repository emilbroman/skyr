<script lang="ts">
let {
    challenge,
    onsubmit,
    submitLabel = "Submit",
    pendingLabel = "Submitting...",
    pending = false,
    showRefreshNote = false,
}: {
    challenge: string;
    onsubmit: (signature: string) => void;
    submitLabel?: string;
    pendingLabel?: string;
    pending?: boolean;
    showRefreshNote?: boolean;
} = $props();

let response = $state("");
let parseError = $state<string | null>(null);

function handleSubmit() {
    const trimmed = response.trim();
    if (
        !trimmed.startsWith("-----BEGIN SSH SIGNATURE-----") ||
        !trimmed.endsWith("-----END SSH SIGNATURE-----")
    ) {
        parseError = "Could not find a valid SSH signature. Make sure you pasted the full output.";
        return;
    }
    parseError = null;
    onsubmit(trimmed);
}

function copyToClipboard(text: string) {
    navigator.clipboard.writeText(text);
}

const clipboardSuffix = $derived.by(() => {
    if (typeof navigator === "undefined") return "";
    const ua = navigator.userAgent;
    if (/Mac/i.test(ua)) return " | pbcopy";
    if (/Win/i.test(ua)) return " | clip.exe";
    if (/Linux/i.test(ua)) return " | xclip -selection clipboard";
    return "";
});

const command = $derived(
    clipboardSuffix
        ? `echo -n '${challenge}' | ssh-keygen -Y sign -f ~/.ssh/id_ed25519 -n skyr-auth-challenge 2>/dev/null${clipboardSuffix}; echo "Signature copied to clipboard"`
        : `echo -n '${challenge}' | ssh-keygen -Y sign -f ~/.ssh/id_ed25519 -n skyr-auth-challenge`,
);
</script>

<div class="space-y-4">
    {#if parseError}
        <p class="text-red-600">{parseError}</p>
    {/if}

    <div>
        <p class="text-gray-600 mb-2">Run this command in your terminal:</p>
        <div class="relative">
            <pre
                class="bg-gray-100 border border-gray-300 rounded p-3 text-green-700 overflow-x-auto whitespace-pre-wrap break-all">{command}</pre>
            <button
                class="absolute top-2 right-2 text-gray-500 hover:text-gray-900 px-2 py-1 bg-gray-200 rounded"
                onclick={() => copyToClipboard(command)}
            >
                Copy
            </button>
        </div>
        <label class="block mt-2 text-gray-500" for="ssh-response">
            Paste the full output:
        </label>
        <textarea
            id="ssh-response"
            bind:value={response}
            placeholder="-----BEGIN SSH SIGNATURE-----&#10;...&#10;-----END SSH SIGNATURE-----"
            rows={8}
            class="w-full mt-1 px-3 py-2 bg-gray-100 border border-gray-300 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-orange-500 font-mono text-xs"
        ></textarea>
    </div>

    {#if showRefreshNote}
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
            <span>
                The challenge auto-refreshes every minute. Make sure to sign the
                latest command shown above.
            </span>
        </div>
    {/if}

    <button
        onclick={handleSubmit}
        class="w-full px-4 py-2 bg-gray-200 hover:bg-gray-300 text-gray-700 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
        disabled={pending || !response.trim()}
    >
        {pending ? pendingLabel : submitLabel}
    </button>
</div>
