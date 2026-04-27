export function formatLogTimestamp(ts: string): string {
    try {
        const date = new Date(ts);
        const now = new Date();
        const sameDay =
            date.getFullYear() === now.getFullYear() &&
            date.getMonth() === now.getMonth() &&
            date.getDate() === now.getDate();
        if (sameDay) {
            return date.toLocaleTimeString();
        }
        return `${date.toLocaleDateString()} ${date.toLocaleTimeString()}`;
    } catch {
        return ts;
    }
}

const SHORT_MONTHS = [
    "Jan",
    "Feb",
    "Mar",
    "Apr",
    "May",
    "Jun",
    "Jul",
    "Aug",
    "Sep",
    "Oct",
    "Nov",
    "Dec",
];

function ordinalSuffix(day: number): string {
    const mod100 = day % 100;
    if (mod100 >= 11 && mod100 <= 13) return "th";
    switch (day % 10) {
        case 1:
            return "st";
        case 2:
            return "nd";
        case 3:
            return "rd";
        default:
            return "th";
    }
}

function pad2(n: number): string {
    return String(n).padStart(2, "0");
}

/**
 * Compact timestamp: time-of-day if same day, "Mon Dth" if same year,
 * ISO date otherwise.
 */
export function formatCompactTimestamp(ts: string, now: Date = new Date()): string {
    const date = new Date(ts);
    if (Number.isNaN(date.getTime())) return ts;
    if (date.getFullYear() === now.getFullYear()) {
        if (date.getMonth() === now.getMonth() && date.getDate() === now.getDate()) {
            return `${pad2(date.getHours())}:${pad2(date.getMinutes())}`;
        }
        const day = date.getDate();
        return `${SHORT_MONTHS[date.getMonth()]} ${day}${ordinalSuffix(day)}`;
    }
    return `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`;
}

/** Human-readable duration like "5m 2s" or "1d 3h 0m 0s". */
export function formatDuration(ms: number): string {
    const totalSeconds = Math.floor(Math.max(0, ms) / 1000);
    const days = Math.floor(totalSeconds / 86400);
    const hours = Math.floor((totalSeconds % 86400) / 3600);
    const minutes = Math.floor((totalSeconds % 3600) / 60);
    const seconds = totalSeconds % 60;
    const parts: string[] = [];
    if (days > 0) parts.push(`${days}d`);
    if (hours > 0 || days > 0) parts.push(`${hours}h`);
    if (minutes > 0 || hours > 0 || days > 0) parts.push(`${minutes}m`);
    parts.push(`${seconds}s`);
    return parts.join(" ");
}
