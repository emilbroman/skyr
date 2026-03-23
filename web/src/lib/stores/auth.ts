import { derived, get, writable } from "svelte/store";
import { browser } from "$app/environment";
import { query } from "$lib/graphql/client";
import type { User } from "$lib/graphql/generated";
import { RefreshTokenDocument } from "$lib/graphql/generated";

const TOKEN_KEY = "skyr_token";
const USER_KEY = "skyr_user";

function loadToken(): string | null {
    if (!browser) return null;
    return localStorage.getItem(TOKEN_KEY);
}

function loadUser(): User | null {
    if (!browser) return null;
    const raw = localStorage.getItem(USER_KEY);
    if (!raw) return null;
    try {
        return JSON.parse(raw);
    } catch {
        return null;
    }
}

function parseTokenExpiry(token: string): number | null {
    if (token.length < 9 || token[8] !== ".") return null;
    const hex = token.substring(0, 8);
    const expiry = parseInt(hex, 16);
    if (Number.isNaN(expiry)) return null;
    return expiry;
}

export const token = writable<string | null>(loadToken());
export const user = writable<User | null>(loadUser());

export const isAuthenticated = derived(token, ($token) => {
    if (!$token) return false;
    const expiry = parseTokenExpiry($token);
    if (!expiry) return false;
    return Date.now() / 1000 < expiry;
});

export function setAuth(newToken: string, newUser: User) {
    token.set(newToken);
    user.set(newUser);
    localStorage.setItem(TOKEN_KEY, newToken);
    localStorage.setItem(USER_KEY, JSON.stringify(newUser));
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
