# Skyr Identity and Access Service (IAS)

IAS is the per-region service that owns each region's identity-token signing key and challenge salt. It is the **only** path to the regional UDB: every API edge talks to IAS over gRPC for sign-up, sign-in, refresh, credential management, and every user/org read.

## Role in the Architecture

```
API edge ──gRPC──▶ IAS (home region) ──▶ UDB (Redis)
              ──▶ IAS (any region) ──▶ GetVerifyingKey (token verification)
```

The API edge is region-agnostic. Per-request routing resolves the user's home region via GDDB, then invokes the appropriate IAS RPC. Token verification on any edge calls `GetVerifyingKey` on the issuing region's IAS (cached short-TTL).

IAS performs:

- **Challenge issuance.** Frame-aligned 60-second windows derived from a per-region salt + username; the API wraps the challenge string into the WebAuthn JSON for the browser, or the user signs it directly with `ssh-keygen -Y sign`.
- **Proof verification.** SSH (`SshSig`) and WebAuthn (CBOR attestation, ES256/Ed25519 signature) both validate against the recent challenge frames inside IAS. The API edge does only the JSON envelope unwrapping.
- **Identity-token signing.** Ed25519 signature over `(username, issuer_region, issued_at, expires_at, nonce)`. The signing key is exclusive to the IAS that owns the user's home region.
- **UDB CRUD.** Users, public keys / WebAuthn credentials, organisations, and org membership.

## Trust Model

IAS RPCs trust the calling edge to have authenticated the bearer token. The service has no per-RPC authentication of its own — same in-mesh trust posture as the rest of Skyr's internal services.

## Protocol

The protocol is defined in `proto/ias.proto`. RPC categories:

| RPCs | Purpose |
|------|---------|
| `GetVerifyingKey` | Publish this region's identity-token signing public key. |
| `IssueChallenge`, `Signup`, `Signin`, `RefreshToken` | Auth flows. |
| `AddCredential`, `RemoveCredential`, `ListCredentials` | Credential management. |
| `GetUser`, `UpdateFullname`, `ListUserOrgs` | User records. |
| `CreateOrg`, `GetOrg`, `ListOrgMembers`, `OrgContainsMember`, `AddOrgMember`, `RemoveOrgMember` | Orgs and membership. |

## Running

```sh
ias \
  --region loca \
  --udb-host redis-loca \
  --signing-key /path/to/udb-signing.key \
  --port 50100
```

`SKYR_CHALLENGE_SALT` (env) provides the challenge salt. The signing-key file is a 32-byte raw Ed25519 secret scalar (generate with `head -c 32 /dev/urandom > udb-signing.key`).

## Related Crates

- [api](../api/) — region-agnostic GraphQL edge; only client of IAS.
- [udb](../udb/) — Redis-backed storage layer used internally by IAS.
- [auth_token](../auth_token/) — wire format and Ed25519 signing/verification of identity tokens.
- [gddb](../gddb/) — global directory used by edges to resolve a username to its home region (and thus to the right IAS).
