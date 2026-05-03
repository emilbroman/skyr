import { derived, get, writable } from "svelte/store";
import { browser } from "$app/environment";
import { query } from "$lib/graphql/client";
import type { SignedInUser } from "$lib/graphql/generated";
import { RefreshTokenDocument } from "$lib/graphql/generated";

const TOKEN_KEY = "skyr_token";
const USER_KEY = "skyr_user";

function loadToken(): string | null {
    if (!browser) return null;
    return localStorage.getItem(TOKEN_KEY);
}

function loadSignedInUser(): SignedInUser | null {
    if (!browser) return null;
    const raw = localStorage.getItem(USER_KEY);
    if (!raw) return null;
    try {
        return JSON.parse(raw);
    } catch {
        return null;
    }
}

// Token wire format: base64url(payload).base64url(signature)
// Payload: u8 version=1, u8 username_len, username, u8 region_len, region,
// i64 issued_at (BE), i64 expires_at (BE), 16 bytes nonce. See crates/auth_token.
function parseTokenExpiry(token: string): number | null {
    const dot = token.indexOf(".");
    if (dot < 0) return null;
    let payload: Uint8Array;
    try {
        payload = Uint8Array.fromBase64(token.substring(0, dot), { alphabet: "base64url" });
    } catch {
        return null;
    }
    let cur = 0;
    if (cur >= payload.length || payload[cur++] !== 1) return null;
    if (cur >= payload.length) return null;
    cur += 1 + payload[cur];
    if (cur >= payload.length) return null;
    cur += 1 + payload[cur];
    cur += 8;
    if (cur + 8 > payload.length) return null;
    const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
    return Number(view.getBigInt64(cur, false));
}

export const token = writable<string | null>(loadToken());
export const user = writable<SignedInUser | null>(loadSignedInUser());

export const isAuthenticated = derived(token, ($token) => {
    if (!$token) return false;
    const expiry = parseTokenExpiry($token);
    if (!expiry) return false;
    return Date.now() / 1000 < expiry;
});

export function setAuth(newToken: string, newSignedInUser: SignedInUser) {
    token.set(newToken);
    user.set(newSignedInUser);
    localStorage.setItem(TOKEN_KEY, newToken);
    localStorage.setItem(USER_KEY, JSON.stringify(newSignedInUser));
}

export function clearAuth() {
    token.set(null);
    user.set(null);
    localStorage.removeItem(TOKEN_KEY);
    localStorage.removeItem(USER_KEY);
}

export function getToken(): string | null {
    return get(token);
}

// Periodically check token expiry and auto-refresh
let expiryInterval: ReturnType<typeof setInterval> | null = null;
let refreshing = false;

async function tryRefreshToken() {
    if (refreshing) return;
    refreshing = true;
    try {
        const data = await query(RefreshTokenDocument);
        setAuth(data.refreshToken.token, data.refreshToken.user);
    } catch {
        // Refresh failed — token may already be invalid
        clearAuth();
    } finally {
        refreshing = false;
    }
}

export function startExpiryWatch() {
    if (expiryInterval) return;
    expiryInterval = setInterval(() => {
        const t = get(token);
        if (!t) return;
        const expiry = parseTokenExpiry(t);
        if (!expiry) return;
        const remaining = expiry - Date.now() / 1000;
        if (remaining <= 0) {
            clearAuth();
        } else if (remaining < 120) {
            tryRefreshToken();
        }
        // Trigger reactivity by re-setting the same value
        // (derived stores will re-evaluate)
        token.set(t);
    }, 30_000);
}

export function stopExpiryWatch() {
    if (expiryInterval) {
        clearInterval(expiryInterval);
        expiryInterval = null;
    }
}
