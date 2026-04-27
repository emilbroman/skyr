//! Lookup org/repo/env defaults from the working directory's git state.
//!
//! These functions shell out to `git`. They are invoked lazily by [`Context`]
//! so that non-API commands (`repl`, `run`, `fmt`, `lsp`) can be used outside
//! a git repository.

use std::process::Command;

use anyhow::{Context, anyhow, bail};

/// Run `git` with the given args in the current working directory and return
/// the trimmed stdout, mapping common failure shapes to user-friendly errors.
fn run_git(args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .context("failed to run `git` (is it installed and on PATH?)")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("git {} failed: {}", args.join(" "), stderr.trim()));
    }
    Ok(String::from_utf8(output.stdout)
        .context("git produced non-utf8 output")?
        .trim()
        .to_owned())
}

pub fn current_org_repo() -> anyhow::Result<(String, String)> {
    let (remote, url) = match run_git(&["remote", "get-url", "skyr"]) {
        Ok(url) => ("skyr", url),
        Err(skyr_err) => match run_git(&["remote", "get-url", "origin"]) {
            Ok(url) => ("origin", url),
            Err(origin_err) => {
                return Err(anyhow!(
                    "could not read `skyr` or `origin` remote:\n  skyr: {skyr_err}\n  origin: {origin_err}"
                ));
            }
        },
    };
    parse_remote_url(&url).with_context(|| format!("could not parse `{remote}` remote url `{url}`"))
}

pub fn current_branch() -> anyhow::Result<String> {
    let branch = run_git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        bail!("HEAD is detached; pass --env explicitly");
    }
    Ok(branch)
}

/// Parse the org/repo pair out of a remote url. Supported shapes:
///
/// - `host[:port]/org/repo[.git]`               (Skyr's bare host:port form)
/// - `git@host:org/repo[.git]`                  (scp-like SSH)
/// - `ssh://[user@]host[:port]/org/repo[.git]`
/// - `https://host[:port]/org/repo[.git]`
/// - `http://host[:port]/org/repo[.git]`
pub fn parse_remote_url(url: &str) -> anyhow::Result<(String, String)> {
    let path = if let Some(rest) = url
        .strip_prefix("ssh://")
        .or_else(|| url.strip_prefix("https://"))
        .or_else(|| url.strip_prefix("http://"))
    {
        // Skip past the authority (everything up to the first `/`).
        let (_authority, path) = rest
            .split_once('/')
            .ok_or_else(|| anyhow!("missing path component"))?;
        path
    } else if let Some(rest) = url.strip_prefix("git@") {
        // git@host:org/repo
        let (_host, path) = rest
            .split_once(':')
            .ok_or_else(|| anyhow!("expected `host:org/repo` after `git@`"))?;
        path
    } else if let Some((authority, path)) = url.split_once('/') {
        // Bare `host[:port]/org/repo`. Disambiguate from a no-host
        // `org/repo` form by requiring something that looks like a host
        // (contains `.` or `:`) before the first slash.
        if !authority.contains('.') && !authority.contains(':') {
            bail!("unrecognised url shape");
        }
        path
    } else {
        bail!("unrecognised url shape");
    };

    let path = path.trim_start_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (org, repo) = path
        .split_once('/')
        .ok_or_else(|| anyhow!("expected `<org>/<repo>` in path"))?;
    if org.is_empty() || repo.is_empty() {
        bail!("org and repo must both be non-empty");
    }
    if repo.contains('/') {
        bail!("path has more than two segments");
    }
    Ok((org.to_owned(), repo.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port_form() {
        assert_eq!(
            parse_remote_url("localhost:2222/test/test").unwrap(),
            ("test".into(), "test".into())
        );
        assert_eq!(
            parse_remote_url("skyr.cloud:22/myorg/myrepo").unwrap(),
            ("myorg".into(), "myrepo".into())
        );
    }

    #[test]
    fn scp_form() {
        assert_eq!(
            parse_remote_url("git@skyr.cloud:myorg/myrepo.git").unwrap(),
            ("myorg".into(), "myrepo".into())
        );
        assert_eq!(
            parse_remote_url("git@skyr.cloud:myorg/myrepo").unwrap(),
            ("myorg".into(), "myrepo".into())
        );
    }

    #[test]
    fn ssh_url_form() {
        assert_eq!(
            parse_remote_url("ssh://git@skyr.cloud:22/myorg/myrepo.git").unwrap(),
            ("myorg".into(), "myrepo".into())
        );
        assert_eq!(
            parse_remote_url("ssh://skyr.cloud/myorg/myrepo").unwrap(),
            ("myorg".into(), "myrepo".into())
        );
    }

    #[test]
    fn https_form() {
        assert_eq!(
            parse_remote_url("https://skyr.cloud/myorg/myrepo.git").unwrap(),
            ("myorg".into(), "myrepo".into())
        );
        assert_eq!(
            parse_remote_url("http://localhost:8080/myorg/myrepo").unwrap(),
            ("myorg".into(), "myrepo".into())
        );
    }

    #[test]
    fn rejects_extra_segments() {
        assert!(parse_remote_url("https://skyr.cloud/a/b/c").is_err());
    }

    #[test]
    fn rejects_missing_repo() {
        assert!(parse_remote_url("https://skyr.cloud/onlyone").is_err());
        assert!(parse_remote_url("git@skyr.cloud:onlyone").is_err());
    }

    #[test]
    fn rejects_bare_relative_path() {
        // Without a `host[:port]` authority we shouldn't guess.
        assert!(parse_remote_url("just/a/path").is_err());
    }
}
