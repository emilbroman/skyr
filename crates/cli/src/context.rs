//! Resolved invocation context shared by API-talking subcommands.
//!
//! `Context` holds the API URL and output format, plus the org/repo/env
//! triple. The triple is resolved lazily: explicit `--org`/`--repo`/`--env`
//! flags (or env vars) win, and any unset member falls back to deriving from
//! git state when first asked. Non-API commands (`repl`, `run`, `fmt`, etc.)
//! never trigger the git lookup, so they keep working outside a git repo.

use std::sync::OnceLock;

use anyhow::{Context as _, anyhow};

use crate::{git_context, output::OutputFormat};

pub struct Context {
    api_url: String,
    pub format: OutputFormat,
    org_override: Option<String>,
    repo_override: Option<String>,
    env_override: Option<String>,
    git_origin: OnceLock<Result<(String, String), String>>,
    git_branch: OnceLock<Result<String, String>>,
}

impl Context {
    pub fn new(
        api_url: String,
        format: OutputFormat,
        org_override: Option<String>,
        repo_override: Option<String>,
        env_override: Option<String>,
    ) -> Self {
        Self {
            api_url,
            format,
            org_override,
            repo_override,
            env_override,
            git_origin: OnceLock::new(),
            git_branch: OnceLock::new(),
        }
    }

    pub fn api_url(&self) -> &str {
        &self.api_url
    }

    pub fn org(&self) -> anyhow::Result<&str> {
        if let Some(o) = &self.org_override {
            return Ok(o);
        }
        match self.origin() {
            Ok((org, _)) => Ok(org),
            Err(e) => Err(anyhow!("could not derive --org from git: {e}")),
        }
    }

    pub fn repo(&self) -> anyhow::Result<&str> {
        if let Some(r) = &self.repo_override {
            return Ok(r);
        }
        match self.origin() {
            Ok((_, repo)) => Ok(repo),
            Err(e) => Err(anyhow!("could not derive --repo from git: {e}")),
        }
    }

    pub fn env(&self) -> anyhow::Result<&str> {
        if let Some(e) = &self.env_override {
            return Ok(e);
        }
        let branch = self
            .git_branch
            .get_or_init(|| git_context::current_branch().map_err(|e| format!("{e:#}")));
        branch
            .as_deref()
            .map_err(|e| anyhow!("could not derive --env from git branch: {e}"))
            .with_context(|| "pass --env explicitly or set SKYR_ENV")
    }

    fn origin(&self) -> Result<(&str, &str), &str> {
        let cached = self
            .git_origin
            .get_or_init(|| git_context::current_org_repo().map_err(|e| format!("{e:#}")));
        match cached {
            Ok((org, repo)) => Ok((org.as_str(), repo.as_str())),
            Err(e) => Err(e.as_str()),
        }
    }
}
