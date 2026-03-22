/**
 * WebAuthn helpers for passkey registration (attestation) and signin (assertion).
 *
 * Uses the standardized Uint8Array.fromBase64() / .toBase64() for base64url encoding.
 * Type declarations are needed because TypeScript doesn't ship these types yet.
 */

declare global {
    interface Uint8Array {
        toBase64(options?: { alphabet?: "base64" | "base64url"; omitPadding?: boolean }): string;
    }
    interface Uint8ArrayConstructor {
        fromBase64(base64: string, options?: { alphabet?: "base64" | "base64url" }): Uint8Array;
    }
}

function toBase64url(buffer: ArrayBuffer): string {
    return new Uint8Array(buffer).toBase64({ alphabet: "base64url", omitPadding: true });
}

function fromBase64url(str: string): ArrayBuffer {
    return Uint8Array.fromBase64(str, { alphabet: "base64url" }).buffer as ArrayBuffer;
}

/**
 * Create a passkey registration (WebAuthn attestation).
 * Takes the `passkeyRegistration` options from the AuthChallenge response,
 * calls navigator.credentials.create(), and returns a proof object for the backend.
 */
export async function createPasskeyRegistration(options: any): Promise<Record<string, unknown>> {
    // Decode base64url fields to ArrayBuffers for the WebAuthn API
    const publicKey = {
        ...options,
        challenge: fromBase64url(options.challenge),
        user: {
            ...options.user,
            id: fromBase64url(options.user.id),
        },
        excludeCredentials: (options.excludeCredentials ?? []).map((cred: any) => ({
            ...cred,
            id: fromBase64url(cred.id),
        })),
    };

    const credential = (await navigator.credentials.create({
        publicKey,
    })) as PublicKeyCredential;
    const response = credential.response as AuthenticatorAttestationResponse;

    return {
        id: toBase64url(credential.rawId),
        response: {
            clientDataJSON: toBase64url(response.clientDataJSON),
            attestationObject: toBase64url(response.attestationObject),
        },
    };
}

/**
 * Create a passkey assertion (WebAuthn authentication).
 * Takes the `passkeySignin` options from the AuthChallenge response,
 * calls navigator.credentials.get(), and returns a proof object for the backend.
 */
export async function createPasskeyAssertion(options: any): Promise<Record<string, unknown>> {
    const publicKey = {
        ...options,
        challenge: fromBase64url(options.challenge),
        allowCredentials: (options.allowCredentials ?? []).map((cred: any) => ({
            ...cred,
            id: fromBase64url(cred.id),
        })),
    };

    const credential = (await navigator.credentials.get({
        publicKey,
    })) as PublicKeyCredential;
    const response = credential.response as AuthenticatorAssertionResponse;

    return {
        id: toBase64url(credential.rawId),
        response: {
            clientDataJSON: toBase64url(response.clientDataJSON),
            authenticatorData: toBase64url(response.authenticatorData),
            signature: toBase64url(response.signature),
        },
    };
}
