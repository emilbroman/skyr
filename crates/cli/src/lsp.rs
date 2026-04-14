use std::path::PathBuf;
use std::sync::Arc;

use lsp::LspTransport;

pub async fn run_lsp(git_server: String) -> anyhow::Result<()> {
    eprintln!("scl language server starting");

    let mut server = lsp::LanguageServer::new();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut transport = LspTransport::new(stdin, stdout);

    let mut deps_resolved = false;

    loop {
        let msg = match transport.read_message().await {
            Ok(msg) => msg,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                eprintln!("client disconnected");
                return Ok(());
            }
            Err(err) => {
                eprintln!("failed to read message: {err}");
                return Err(err.into());
            }
        };

        // After the first didOpen sets the workspace root, resolve
        // dependencies before processing further messages.
        let is_did_open = matches!(
            &msg,
            lsp::IncomingMessage::Notification { method, .. } if method == "textDocument/didOpen"
        );

        let responses = server.handle(msg).await;
        for response in responses {
            if let Err(err) = transport.write_message(response).await {
                eprintln!("failed to write message: {err}");
                return Err(err.into());
            }
        }

        if is_did_open && !deps_resolved {
            deps_resolved = true;
            if let Some(root) = server.root().cloned()
                && resolve_and_set_deps(&mut server, root, &git_server).await
            {
                // Re-publish diagnostics now that deps are available.
                let refreshed = server.refresh_diagnostics().await;
                for msg in refreshed {
                    if let Err(err) = transport.write_message(msg).await {
                        eprintln!("failed to write message: {err}");
                        return Err(err.into());
                    }
                }
            }
        }

        if let Some(code) = server.exit_code() {
            std::process::exit(code);
        }
    }
}

/// Resolve Package.scle dependencies and set them on the LSP server.
///
/// Uses the same caching mechanism as `skyr run` / `skyr repl`.
/// Errors are logged but non-fatal — the LSP continues without
/// cross-repo dependency resolution.
///
/// Returns `true` if any dependencies were resolved.
async fn resolve_and_set_deps(
    server: &mut lsp::LanguageServer,
    root: PathBuf,
    git_server: &str,
) -> bool {
    let package_id = server.package_id().clone();
    match resolve_deps(&root, &package_id, git_server).await {
        Ok(finders) if !finders.is_empty() => {
            eprintln!(
                "lsp: resolved {} package dependenc{}",
                finders.len(),
                if finders.len() == 1 { "y" } else { "ies" },
            );
            server.set_cached_dep_finders(finders);
            true
        }
        Ok(_) => false,
        Err(err) => {
            eprintln!("lsp: failed to resolve dependencies: {err:#}");
            false
        }
    }
}

async fn resolve_deps(
    root: &std::path::Path,
    package_id: &sclc::PackageId,
    git_server: &str,
) -> anyhow::Result<Vec<Arc<dyn sclc::PackageFinder>>> {
    let user_package: Arc<dyn sclc::Package> =
        Arc::new(sclc::FsPackage::new(root.to_path_buf(), package_id.clone()));
    let default_finder = sclc::build_default_finder(Arc::clone(&user_package));

    let manifest = sclc::load_manifest(Arc::clone(&user_package), default_finder.clone()).await?;
    let Some(manifest) = manifest else {
        return Ok(Vec::new());
    };
    if manifest.dependencies.is_empty() {
        return Ok(Vec::new());
    }

    let git_client = crate::git_client::GitClient::from_config(git_server.to_string()).await?;

    let resolved =
        crate::resolver::resolve_all(Arc::clone(&user_package), default_finder, &git_client)
            .await?;

    let finders: Vec<Arc<dyn sclc::PackageFinder>> = resolved
        .into_iter()
        .map(|rp| {
            let dep_pkg = sclc::FsPackage::new(rp.cache_dir, rp.package_id);
            sclc::wrap_as_finder(Arc::new(dep_pkg))
        })
        .collect();

    Ok(finders)
}
