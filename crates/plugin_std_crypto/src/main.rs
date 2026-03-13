use clap::Parser;
use sclc::ValueAssertions;
use tracing::info;

const ED25519_RESOURCE_TYPE: &str = "Std/Crypto.ED25519PrivateKey";
const ECDSA_RESOURCE_TYPE: &str = "Std/Crypto.ECDSAPrivateKey";
const RSA_RESOURCE_TYPE: &str = "Std/Crypto.RSAPrivateKey";

#[derive(Parser)]
struct Args {
    #[arg(long)]
    bind: String,
}

struct CryptoPlugin;

impl CryptoPlugin {
    fn new() -> Self {
        Self
    }

    async fn dispatch(
        &self,
        id: &sclc::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let (private_pem, public_pem) = match id.ty.as_str() {
            ED25519_RESOURCE_TYPE => generate_ed25519()?,
            ECDSA_RESOURCE_TYPE => {
                let curve = inputs.get("curve").assert_str_ref()?;
                generate_ecdsa(curve)?
            }
            RSA_RESOURCE_TYPE => {
                let size = *inputs.get("size").assert_int_ref()? as usize;
                if size < 2048 {
                    anyhow::bail!("RSA key size must be at least 2048, got {size}");
                }
                tokio::task::spawn_blocking(move || generate_rsa(size)).await??
            }
            _ => anyhow::bail!("unsupported resource type: {}", id.ty),
        };

        info!(
            resource_type = id.ty.as_str(),
            resource_id = id.id.as_str(),
            "generated key pair"
        );

        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("pem"), sclc::Value::Str(private_pem));
        outputs.insert(String::from("publicKeyPem"), sclc::Value::Str(public_pem));

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
        })
    }
}

fn generate_ed25519() -> anyhow::Result<(String, String)> {
    use ed25519_dalek::SigningKey;
    use pkcs8::EncodePrivateKey;
    use spki::EncodePublicKey;

    let signing_key = SigningKey::generate(&mut rand_core::OsRng);
    let private_pem = signing_key.to_pkcs8_pem(pkcs8::LineEnding::LF)?.to_string();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(pkcs8::LineEnding::LF)?;

    Ok((private_pem, public_pem))
}

fn generate_ecdsa(curve: &str) -> anyhow::Result<(String, String)> {
    use pkcs8::EncodePrivateKey;
    use spki::EncodePublicKey;

    match curve {
        "P-256" => {
            let key = p256::SecretKey::random(&mut rand_core::OsRng);
            let private_pem = key.to_pkcs8_pem(pkcs8::LineEnding::LF)?.to_string();
            let public_pem = key.public_key().to_public_key_pem(pkcs8::LineEnding::LF)?;
            Ok((private_pem, public_pem))
        }
        "P-384" => {
            let key = p384::SecretKey::random(&mut rand_core::OsRng);
            let private_pem = key.to_pkcs8_pem(pkcs8::LineEnding::LF)?.to_string();
            let public_pem = key.public_key().to_public_key_pem(pkcs8::LineEnding::LF)?;
            Ok((private_pem, public_pem))
        }
        "P-521" => {
            let key = p521::SecretKey::random(&mut rand_core::OsRng);
            let private_pem = key.to_pkcs8_pem(pkcs8::LineEnding::LF)?.to_string();
            let public_pem = key.public_key().to_public_key_pem(pkcs8::LineEnding::LF)?;
            Ok((private_pem, public_pem))
        }
        _ => anyhow::bail!("unsupported ECDSA curve: {curve} (expected P-256, P-384, or P-521)"),
    }
}

fn generate_rsa(size: usize) -> anyhow::Result<(String, String)> {
    use pkcs8::EncodePrivateKey;
    use spki::EncodePublicKey;

    let private_key = rsa::RsaPrivateKey::new(&mut rand_core::OsRng, size)?;
    let private_pem = private_key.to_pkcs8_pem(pkcs8::LineEnding::LF)?.to_string();
    let public_pem = private_key
        .to_public_key()
        .to_public_key_pem(pkcs8::LineEnding::LF)?;

    Ok((private_pem, public_pem))
}

#[async_trait::async_trait]
impl rtp::Plugin for CryptoPlugin {
    async fn create_resource(
        &mut self,
        _environment_qid: &str,
        _deployment_id: &str,
        id: sclc::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        self.dispatch(&id, inputs).await
    }

    async fn update_resource(
        &mut self,
        _environment_qid: &str,
        _deployment_id: &str,
        id: sclc::ResourceId,
        _prev_inputs: sclc::Record,
        _prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        self.dispatch(&id, inputs).await
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    rtp::serve(&args.bind, CryptoPlugin::new).await?;
    Ok(())
}
