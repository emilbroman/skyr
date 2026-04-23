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
