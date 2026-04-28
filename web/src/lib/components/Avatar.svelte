<script lang="ts">
type Size = "xs" | "sm" | "md" | "lg";

let {
    src,
    username,
    fullname,
    size = "md",
    class: className = "",
}: {
    src?: string | null;
    username: string;
    fullname?: string | null;
    size?: Size;
    class?: string;
} = $props();

const sizeClasses: Record<Size, string> = {
    xs: "w-5 h-5 text-[9px]",
    sm: "w-6 h-6 text-[10px]",
    md: "w-8 h-8 text-xs",
    lg: "w-10 h-10 text-sm",
};

const palette = [
    "bg-orange-600",
    "bg-amber-600",
    "bg-emerald-600",
    "bg-teal-600",
    "bg-sky-600",
    "bg-blue-600",
    "bg-indigo-600",
    "bg-violet-600",
    "bg-fuchsia-600",
    "bg-pink-600",
    "bg-rose-600",
];

function initials(value: string): string {
    const trimmed = value.trim();
    if (!trimmed) return "?";
    const parts = trimmed.split(/\s+/).filter(Boolean);
    if (parts.length >= 2) {
        return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
    }
    return trimmed.slice(0, 2).toUpperCase();
}

function colorFor(value: string): string {
    let h = 0;
    for (let i = 0; i < value.length; i++) {
        h = (h * 31 + value.charCodeAt(i)) | 0;
    }
    return palette[Math.abs(h) % palette.length];
}

let bg = $derived(colorFor(username));
let text = $derived(initials(fullname || username));
let label = $derived(fullname ? `${fullname} (@${username})` : `@${username}`);
</script>

{#if src}
    <img
        {src}
        alt={label}
        class="{sizeClasses[size]} rounded-full object-cover shrink-0 {className}"
    />
{:else}
    <div
        aria-label={label}
        class="{sizeClasses[size]} {bg} rounded-full flex items-center justify-center font-semibold text-white shrink-0 select-none {className}"
    >
        {text}
    </div>
{/if}
