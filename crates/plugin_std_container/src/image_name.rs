//! Image name qualification for the container registry.
//!
//! Skyr orgs, repos, and image input names follow SCL's conventional
//! PascalCase/snake_case styles, but OCI image path components must be
//! lowercase with only `[a-z0-9._-]`. This module converts each segment to
//! kebab-case and prefixes the result with a short SHA-1 hash of the
//! original `Org/Repo/name` string so that inputs whose kebab forms would
//! otherwise collide (e.g. `MyOrg` vs. `my_org`) still produce distinct
//! registry paths.

use sha1::Digest;

/// Number of leading hex characters of the SHA-1 digest to include as the
/// disambiguating prefix.
const HASH_PREFIX_LEN: usize = 8;

/// Build the qualified registry path for an image belonging to `org/repo`
/// with input `name`. The result is of the form
/// `{hash}/{kebab-org}/{kebab-repo}/{kebab-name}`.
pub(crate) fn qualify(org: &str, repo: &str, name: &str) -> String {
    let proper = format!("{org}/{repo}/{name}");
    let digest = sha1::Sha1::digest(proper.as_bytes());
    let mut hash_prefix = String::with_capacity(HASH_PREFIX_LEN);
    for byte in digest.iter().take(HASH_PREFIX_LEN.div_ceil(2)) {
        hash_prefix.push_str(&format!("{byte:02x}"));
    }
    hash_prefix.truncate(HASH_PREFIX_LEN);

    format!(
        "{hash_prefix}/{}/{}/{}",
        to_kebab(org),
        to_kebab(repo),
        to_kebab(name),
    )
}

/// Convert `s` to kebab-case suitable for an OCI image path component.
///
/// Word boundaries are detected between lowercase/digit → uppercase (e.g.
/// `camelCase` → `camel-case`) and between runs of uppercase letters and a
/// following lowercase letter (e.g. `HTTPServer` → `http-server`).
/// Underscores, spaces, dots, and hyphens are treated as explicit separators.
/// Any other non-alphanumeric character is dropped; the hash prefix produced
/// by [`qualify`] carries the disambiguation for the dropped characters.
fn to_kebab(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());

    for (i, &c) in chars.iter().enumerate() {
        if matches!(c, '_' | '-' | ' ' | '.') {
            push_separator(&mut out);
            continue;
        }
        if !c.is_ascii_alphanumeric() {
            continue;
        }
        if c.is_ascii_uppercase() {
            let prev = i.checked_sub(1).map(|j| chars[j]);
            let next = chars.get(i + 1).copied();
            let boundary = match prev {
                None => false,
                Some(p) if p.is_ascii_lowercase() || p.is_ascii_digit() => true,
                Some(p) if p.is_ascii_uppercase() => next.is_some_and(|n| n.is_ascii_lowercase()),
                _ => false,
            };
            if boundary {
                push_separator(&mut out);
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }

    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn push_separator(out: &mut String) {
    if !out.is_empty() && !out.ends_with('-') {
        out.push('-');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kebab_pascal_case() {
        assert_eq!(to_kebab("MyOrg"), "my-org");
        assert_eq!(to_kebab("MyRepo"), "my-repo");
        assert_eq!(to_kebab("PascalCase"), "pascal-case");
    }

    #[test]
    fn kebab_camel_case() {
        assert_eq!(to_kebab("myOrg"), "my-org");
        assert_eq!(to_kebab("camelCaseName"), "camel-case-name");
    }

    #[test]
    fn kebab_snake_case() {
        assert_eq!(to_kebab("my_image_name"), "my-image-name");
        assert_eq!(to_kebab("snake_case"), "snake-case");
    }

    #[test]
    fn kebab_acronyms() {
        assert_eq!(to_kebab("HTTPServer"), "http-server");
        assert_eq!(to_kebab("URLParser"), "url-parser");
        assert_eq!(to_kebab("HTTP"), "http");
    }

    #[test]
    fn kebab_digits() {
        assert_eq!(to_kebab("Repo2"), "repo2");
        assert_eq!(to_kebab("Repo2Go"), "repo2-go");
        assert_eq!(to_kebab("v2Api"), "v2-api");
    }

    #[test]
    fn kebab_already_kebab() {
        assert_eq!(to_kebab("already-kebab"), "already-kebab");
        assert_eq!(to_kebab("with-many-parts"), "with-many-parts");
    }

    #[test]
    fn kebab_edge_cases() {
        assert_eq!(to_kebab("UPPER"), "upper");
        assert_eq!(to_kebab("lower"), "lower");
        assert_eq!(to_kebab("_leading"), "leading");
        assert_eq!(to_kebab("trailing_"), "trailing");
        assert_eq!(to_kebab("a__b"), "a-b");
        assert_eq!(to_kebab("dots.and.more"), "dots-and-more");
        assert_eq!(to_kebab("with spaces"), "with-spaces");
    }

    #[test]
    fn qualify_produces_expected_shape() {
        let q = qualify("MyOrg", "MyRepo", "my_image_name");
        let (hash, rest) = q.split_once('/').unwrap();
        assert_eq!(hash.len(), HASH_PREFIX_LEN);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(rest, "my-org/my-repo/my-image-name");
    }

    #[test]
    fn qualify_disambiguates_collisions() {
        // Different originals whose kebab forms would collide must still
        // produce different qualified names thanks to the hash prefix.
        let a = qualify("MyOrg", "Repo", "img");
        let b = qualify("my_org", "Repo", "img");
        assert_ne!(a, b);
        assert_eq!(&a[HASH_PREFIX_LEN..], &b[HASH_PREFIX_LEN..]);
    }

    #[test]
    fn qualify_is_deterministic() {
        assert_eq!(
            qualify("MyOrg", "MyRepo", "my_image_name"),
            qualify("MyOrg", "MyRepo", "my_image_name"),
        );
    }
}
