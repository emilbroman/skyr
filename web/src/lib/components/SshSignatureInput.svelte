<script lang="ts">
import { copyText } from "$lib/clipboard";
import { Info } from "lucide-svelte";

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
    copyText(text);
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

<div class="space-y-3">
    {#if parseError}
        <p class="text-xs text-red-600">{parseError}</p>
    {/if}

    <div>
        <p class="text-xs text-gray-500 mb-1.5">Run this command in your terminal:</p>
        <div class="relative">
            <pre
                class="bg-gray-50 border border-gray-200 rounded p-2.5 text-xs text-gray-800 font-mono overflow-x-auto whitespace-pre-wrap break-all">{command}</pre>
            <button
                class="absolute top-1.5 right-1.5 text-xs text-gray-500 hover:text-gray-900 px-1.5 py-0.5 bg-white border border-gray-200 rounded hover:border-gray-300"
                onclick={() => copyToClipboard(command)}
            >
                Copy
            </button>
        </div>
        <label class="block mt-2 text-xs text-gray-500" for="ssh-response">
            Paste the full output:
        </label>
        <textarea
            id="ssh-response"
            bind:value={response}
            placeholder="-----BEGIN SSH SIGNATURE-----&#10;...&#10;-----END SSH SIGNATURE-----"
            rows={6}
            class="w-full mt-1 px-2.5 py-1.5 bg-white border border-gray-200 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-blue-500 font-mono text-xs"
        ></textarea>
    </div>

    {#if showRefreshNote}
        <div class="flex items-start gap-1.5 text-xs text-gray-400">
            <Info class="w-3.5 h-3.5 shrink-0 mt-0.5" />
            <span>
                The challenge auto-refreshes every minute. Make sure to sign the
                latest command shown above.
            </span>
        </div>
    {/if}

    <button
        onclick={handleSubmit}
        class="w-full px-3 py-1.5 text-xs font-medium text-white bg-gray-900 rounded hover:bg-gray-800 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
        disabled={pending || !response.trim()}
    >
        {pending ? pendingLabel : submitLabel}
    </button>
</div>
