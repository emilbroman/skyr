use clap::Parser;
use sclc::ValueAssertions;
use tracing::info;

const ED25519_RESOURCE_TYPE: &str = "Std/Crypto.ED25519PrivateKey";
const ECDSA_RESOURCE_TYPE: &str = "Std/Crypto.ECDSAPrivateKey";
const RSA_RESOURCE_TYPE: &str = "Std/Crypto.RSAPrivateKey";
const CSR_RESOURCE_TYPE: &str = "Std/Crypto.CertificationRequest";
const CERT_SIG_RESOURCE_TYPE: &str = "Std/Crypto.CertificateSignature";

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
        id: &ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        if id.typ == CSR_RESOURCE_TYPE {
            return self.dispatch_csr(id, inputs).await;
        }

        if id.typ == CERT_SIG_RESOURCE_TYPE {
            return self.dispatch_cert_sig(id, inputs).await;
        }

        let (private_pem, public_pem) = match id.typ.as_str() {
            ED25519_RESOURCE_TYPE => generate_ed25519()?,
            ECDSA_RESOURCE_TYPE => {
                let curve = inputs.get("curve").assert_str_ref()?;
                generate_ecdsa(curve)?
            }
            RSA_RESOURCE_TYPE => {
                let size_i64 = *inputs.get("size").assert_int_ref()?;
                let size: usize = size_i64.try_into().map_err(|_| {
                    anyhow::anyhow!("RSA key size must be a positive integer, got {size_i64}")
                })?;
                if size < 2048 {
                    anyhow::bail!("RSA key size must be at least 2048, got {size}");
                }
                if size > 16384 {
                    anyhow::bail!("RSA key size must be at most 16384, got {size}");
                }
                tokio::task::spawn_blocking(move || generate_rsa(size)).await??
            }
            _ => anyhow::bail!("unsupported resource type: {}", id.typ),
        };

        info!(
            resource_type = id.typ.as_str(),
            resource_name = id.name.as_str(),
            "generated key pair"
        );

        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("pem"), sclc::Value::Str(private_pem));
        outputs.insert(String::from("publicKeyPem"), sclc::Value::Str(public_pem));

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: Default::default(),
        })
    }

    async fn dispatch_csr(
        &self,
        id: &ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let inputs_clone = inputs.clone();
        let csr_pem = tokio::task::spawn_blocking(move || generate_csr(&inputs_clone)).await??;

        info!(
            resource_type = id.typ.as_str(),
            resource_name = id.name.as_str(),
            "generated certification request"
        );

        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("pem"), sclc::Value::Str(csr_pem));

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: Default::default(),
        })
    }

    async fn dispatch_cert_sig(
        &self,
        id: &ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let inputs_clone = inputs.clone();
        let cert_pem =
            tokio::task::spawn_blocking(move || sign_certificate(&inputs_clone)).await??;

        info!(
            resource_type = id.typ.as_str(),
            resource_name = id.name.as_str(),
            "signed certificate"
        );

        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("pem"), sclc::Value::Str(cert_pem));

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: Default::default(),
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

/// Build a CSR using the given signer, subject name, and inputs (for extensions).
///
/// Uses the higher-level `build` API for signers whose signature type implements
/// `SignatureBitStringEncoding` (ECDSA, RSA).
fn build_and_sign_csr<S, Sig>(
    signer: &S,
    name: x509_cert::name::Name,
    inputs: &sclc::Record,
) -> anyhow::Result<String>
where
    S: signature::Keypair + spki::DynSignatureAlgorithmIdentifier + signature::Signer<Sig>,
    S::VerifyingKey: spki::EncodePublicKey,
    Sig: spki::SignatureBitStringEncoding,
{
    use der::EncodePem;
    use x509_cert::builder::{Builder, RequestBuilder};

    let mut builder = RequestBuilder::new(name, signer)?;
    add_csr_extensions(&mut builder, inputs)?;
    let csr = builder.build::<Sig>()?;
    Ok(csr.to_pem(der::pem::LineEnding::LF)?)
}

/// Build a CSR using an Ed25519 signer.
///
/// Ed25519 signatures don't implement `SignatureBitStringEncoding`, so we use
/// the lower-level `finalize`/`assemble` API from the `Builder` trait.
fn build_and_sign_csr_ed25519(
    signer: &ed25519_dalek::SigningKey,
    name: x509_cert::name::Name,
    inputs: &sclc::Record,
) -> anyhow::Result<String> {
    use der::EncodePem;
    use signature::Signer;
    use x509_cert::builder::{Builder, RequestBuilder};

    let mut builder = RequestBuilder::new(name, signer)?;
    add_csr_extensions(&mut builder, inputs)?;

    let blob = builder.finalize()?;
    let sig: ed25519_dalek::Signature = signer.sign(&blob);
    let bit_string = der::asn1::BitString::from_bytes(&sig.to_bytes())?;
    let csr = builder.assemble(bit_string)?;
    Ok(csr.to_pem(der::pem::LineEnding::LF)?)
}

fn generate_csr(inputs: &sclc::Record) -> anyhow::Result<String> {
    use pkcs8::DecodePrivateKey;

    let private_key_pem = inputs.get("privateKeyPem").assert_str_ref()?;
    let subject = inputs.get("subject").assert_record_ref()?;
    let name = build_subject_name(subject)?;

    // Try Ed25519
    if let Ok(signing_key) = ed25519_dalek::SigningKey::from_pkcs8_pem(private_key_pem) {
        return build_and_sign_csr_ed25519(&signing_key, name, inputs);
    }

    // Try ECDSA P-256
    if let Ok(secret_key) = p256::SecretKey::from_pkcs8_pem(private_key_pem) {
        let signing_key = p256::ecdsa::SigningKey::from(secret_key);
        return build_and_sign_csr::<_, p256::ecdsa::DerSignature>(&signing_key, name, inputs);
    }

    // Try ECDSA P-384
    if let Ok(secret_key) = p384::SecretKey::from_pkcs8_pem(private_key_pem) {
        let signing_key = p384::ecdsa::SigningKey::from(secret_key);
        return build_and_sign_csr::<_, p384::ecdsa::DerSignature>(&signing_key, name, inputs);
    }

    // P-521 key detection: fail with a clear message since the p521 crate doesn't
    // yet support the traits required by the x509-cert builder.
    if p521::SecretKey::from_pkcs8_pem(private_key_pem).is_ok() {
        anyhow::bail!("P-521 keys are not yet supported for certification requests");
    }

    // Try RSA
    if let Ok(private_key) = rsa::RsaPrivateKey::from_pkcs8_pem(private_key_pem) {
        let signing_key = rsa::pkcs1v15::SigningKey::<sha2::Sha256>::new(private_key);
        return build_and_sign_csr::<_, rsa::pkcs1v15::Signature>(&signing_key, name, inputs);
    }

    anyhow::bail!("unsupported private key type in PEM")
}

fn epoch_millis_to_x509_time(epoch_millis: i64) -> anyhow::Result<x509_cert::time::Time> {
    use std::time::{Duration, UNIX_EPOCH};

    if epoch_millis < 0 {
        anyhow::bail!("epoch milliseconds must be non-negative, got {epoch_millis}");
    }
    let duration = Duration::from_millis(epoch_millis as u64);
    let system_time = UNIX_EPOCH + duration;

    // x509_cert::time::Time implements From<SystemTime> (via the "std" feature
    // on the der crate, which x509-cert enables through its "builder" feature).
    // Under the hood this picks UtcTime for years ≤ 2049, GeneralizedTime otherwise.
    x509_cert::time::Time::try_from(system_time).map_err(|e| anyhow::anyhow!("invalid time: {e}"))
}

fn sign_certificate(inputs: &sclc::Record) -> anyhow::Result<String> {
    use der::{DecodePem, Encode};
    use pkcs8::DecodePrivateKey;
    use rand_core::RngCore;
    use spki::EncodePublicKey;

    let csr_pem = inputs.get("csrPem").assert_str_ref()?;
    let private_key_pem = inputs.get("privateKeyPem").assert_str_ref()?;

    let ca_cert_pem: Option<&str> = match inputs.get("caCertPem") {
        sclc::Value::Nil => None,
        other => Some(other.assert_str_ref()?),
    };

    let validity_record = inputs.get("validity").assert_record_ref()?;
    let before_record = validity_record.get("before").assert_record_ref()?;
    let not_after_millis = *before_record.get("epochMillis").assert_int_ref()?;
    let not_after = epoch_millis_to_x509_time(not_after_millis)?;

    let not_before_millis = match validity_record.get("after") {
        sclc::Value::Nil => None,
        other => {
            let after_record = other.assert_record_ref()?;
            Some(*after_record.get("epochMillis").assert_int_ref()?)
        }
    };

    if let Some(nb_millis) = not_before_millis
        && nb_millis > not_after_millis
    {
        anyhow::bail!(
            "certificate validity period is invalid: notBefore must not be after notAfter"
        );
    }

    let not_before = match not_before_millis {
        Some(millis) => epoch_millis_to_x509_time(millis)?,
        None => x509_cert::time::Time::try_from(std::time::SystemTime::now())
            .map_err(|e| anyhow::anyhow!("failed to get current time: {e}"))?,
    };

    let validity = x509_cert::time::Validity {
        not_before,
        not_after,
    };

    // Generate random serial number (20 bytes, positive)
    let mut serial_bytes = [0u8; 20];
    rand_core::OsRng.fill_bytes(&mut serial_bytes);
    serial_bytes[0] &= 0x7F; // Ensure positive (clear sign bit)
    let serial_number = x509_cert::serial_number::SerialNumber::new(&serial_bytes)
        .map_err(|e| anyhow::anyhow!("failed to create serial number: {e}"))?;

    // Parse the CSR
    let csr = x509_cert::request::CertReq::from_pem(csr_pem)?;
    let subject = csr.info.subject.clone();
    let subject_pub_key_info = csr.info.public_key.clone();

    // Parse the CA certificate if provided
    let ca_cert = match ca_cert_pem {
        Some(pem) => Some(x509_cert::Certificate::from_pem(pem)?),
        None => None,
    };

    // Determine profile and issuer
    let (profile, _issuer_name) = match &ca_cert {
        Some(cert) => {
            let issuer = cert.tbs_certificate.subject.clone();
            (
                x509_cert::builder::Profile::Leaf {
                    issuer: issuer.clone(),
                    enable_key_agreement: false,
                    enable_key_encipherment: false,
                },
                issuer,
            )
        }
        None => {
            // Self-signed: issuer = subject
            (x509_cert::builder::Profile::Root, subject.clone())
        }
    };

    let csr_spki_der = subject_pub_key_info.to_der()?;
    let ca_spki_der = ca_cert
        .as_ref()
        .map(|cert| cert.tbs_certificate.subject_public_key_info.to_der())
        .transpose()?;

    // Try Ed25519
    if let Ok(signing_key) = ed25519_dalek::SigningKey::from_pkcs8_pem(private_key_pem) {
        let pub_key_der = signing_key.verifying_key().to_public_key_der()?;
        verify_signing_key_match(
            pub_key_der.as_bytes(),
            &csr_spki_der,
            ca_spki_der.as_deref(),
        )?;
        return build_and_sign_cert_ed25519(
            &signing_key,
            profile,
            serial_number,
            validity,
            subject,
            subject_pub_key_info,
        );
    }

    // Try ECDSA P-256
    if let Ok(secret_key) = p256::SecretKey::from_pkcs8_pem(private_key_pem) {
        let pub_key_der = secret_key.public_key().to_public_key_der()?;
        verify_signing_key_match(
            pub_key_der.as_bytes(),
            &csr_spki_der,
            ca_spki_der.as_deref(),
        )?;
        let signing_key = p256::ecdsa::SigningKey::from(secret_key);
        return build_and_sign_cert::<_, p256::ecdsa::DerSignature>(
            &signing_key,
            profile,
            serial_number,
            validity,
            subject,
            subject_pub_key_info,
        );
    }

    // Try ECDSA P-384
    if let Ok(secret_key) = p384::SecretKey::from_pkcs8_pem(private_key_pem) {
        let pub_key_der = secret_key.public_key().to_public_key_der()?;
        verify_signing_key_match(
            pub_key_der.as_bytes(),
            &csr_spki_der,
            ca_spki_der.as_deref(),
        )?;
        let signing_key = p384::ecdsa::SigningKey::from(secret_key);
        return build_and_sign_cert::<_, p384::ecdsa::DerSignature>(
            &signing_key,
            profile,
            serial_number,
            validity,
            subject,
            subject_pub_key_info,
        );
    }

    // P-521 detection
    if p521::SecretKey::from_pkcs8_pem(private_key_pem).is_ok() {
        anyhow::bail!("P-521 keys are not yet supported for certificate signing");
    }

    // Try RSA
    if let Ok(private_key) = rsa::RsaPrivateKey::from_pkcs8_pem(private_key_pem) {
        let pub_key_der = private_key.to_public_key().to_public_key_der()?;
        verify_signing_key_match(
            pub_key_der.as_bytes(),
            &csr_spki_der,
            ca_spki_der.as_deref(),
        )?;
        let signing_key = rsa::pkcs1v15::SigningKey::<sha2::Sha256>::new(private_key);
        return build_and_sign_cert::<_, rsa::pkcs1v15::Signature>(
            &signing_key,
            profile,
            serial_number,
            validity,
            subject,
            subject_pub_key_info,
        );
    }

    anyhow::bail!("unsupported private key type in PEM")
}

/// Verify that the signing private key matches either the CSR's public key (self-signed)
/// or the CA certificate's public key (CA-signed).
fn verify_signing_key_match(
    signing_pub_der: &[u8],
    csr_spki_der: &[u8],
    ca_spki_der: Option<&[u8]>,
) -> anyhow::Result<()> {
    match ca_spki_der {
        Some(ca_der) => {
            if signing_pub_der != ca_der {
                anyhow::bail!(
                    "CA-signed certificate requested but the signing private key does not match the CA certificate's public key"
                );
            }
        }
        None => {
            if signing_pub_der != csr_spki_der {
                anyhow::bail!(
                    "self-signed certificate requested but CSR public key does not match the provided private key"
                );
            }
        }
    }
    Ok(())
}

/// Build and sign a certificate using a signer whose signature type implements
/// `SignatureBitStringEncoding` (ECDSA, RSA).
fn build_and_sign_cert<S, Sig>(
    signer: &S,
    profile: x509_cert::builder::Profile,
    serial_number: x509_cert::serial_number::SerialNumber,
    validity: x509_cert::time::Validity,
    subject: x509_cert::name::Name,
    subject_pub_key_info: spki::SubjectPublicKeyInfoOwned,
) -> anyhow::Result<String>
where
    S: signature::Keypair + spki::DynSignatureAlgorithmIdentifier + signature::Signer<Sig>,
    S::VerifyingKey: spki::EncodePublicKey,
    Sig: spki::SignatureBitStringEncoding,
{
    use der::EncodePem;
    use x509_cert::builder::{Builder, CertificateBuilder};

    let builder = CertificateBuilder::new(
        profile,
        serial_number,
        validity,
        subject,
        subject_pub_key_info,
        signer,
    )?;
    let cert = builder.build::<Sig>()?;
    Ok(cert.to_pem(der::pem::LineEnding::LF)?)
}

/// Build and sign a certificate using an Ed25519 signer (which doesn't implement
/// `SignatureBitStringEncoding`), using the lower-level finalize/assemble API.
fn build_and_sign_cert_ed25519(
    signer: &ed25519_dalek::SigningKey,
    profile: x509_cert::builder::Profile,
    serial_number: x509_cert::serial_number::SerialNumber,
    validity: x509_cert::time::Validity,
    subject: x509_cert::name::Name,
    subject_pub_key_info: spki::SubjectPublicKeyInfoOwned,
) -> anyhow::Result<String> {
    use der::EncodePem;
    use signature::Signer;
    use x509_cert::builder::{Builder, CertificateBuilder};

    let mut builder = CertificateBuilder::new(
        profile,
        serial_number,
        validity,
        subject,
        subject_pub_key_info,
        signer,
    )?;

    let blob = builder.finalize()?;
    let sig: ed25519_dalek::Signature = signer.sign(&blob);
    let bit_string = der::asn1::BitString::from_bytes(&sig.to_bytes())?;
    let cert = builder.assemble(bit_string)?;
    Ok(cert.to_pem(der::pem::LineEnding::LF)?)
}

fn build_subject_name(subject: &sclc::Record) -> anyhow::Result<x509_cert::name::Name> {
    let cn = subject.get("commonName").assert_str_ref()?;

    // Build RFC 4514 distinguished name string, most-specific-first
    let mut parts = vec![format!("CN={}", rfc4514_escape(cn))];

    if let Ok(ou) = subject.get("organizationalUnit").assert_str_ref() {
        parts.push(format!("OU={}", rfc4514_escape(ou)));
    }
    if let Ok(o) = subject.get("organization").assert_str_ref() {
        parts.push(format!("O={}", rfc4514_escape(o)));
    }
    if let Ok(l) = subject.get("locality").assert_str_ref() {
        parts.push(format!("L={}", rfc4514_escape(l)));
    }
    if let Ok(st) = subject.get("state").assert_str_ref() {
        parts.push(format!("ST={}", rfc4514_escape(st)));
    }
    if let Ok(c) = subject.get("country").assert_str_ref() {
        parts.push(format!("C={}", rfc4514_escape(c)));
    }

    let dn_string = parts.join(",");
    dn_string
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid distinguished name: {e}"))
}

fn rfc4514_escape(s: &str) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(s.len());
    for (i, c) in s.chars().enumerate() {
        match c {
            '"' | '+' | ',' | ';' | '<' | '>' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '#' if i == 0 => {
                out.push('\\');
                out.push(c);
            }
            ' ' if i == 0 || i == s.len() - 1 => {
                out.push('\\');
                out.push(c);
            }
            // Escape NUL and control characters as hex pairs per RFC 4514 section 2.4
            '\0' => out.push_str("\\00"),
            c if c.is_ascii_control() => {
                let _ = write!(out, "\\{:02X}", c as u32);
            }
            _ => out.push(c),
        }
    }
    out
}

fn add_csr_extensions<S>(
    builder: &mut x509_cert::builder::RequestBuilder<'_, S>,
    inputs: &sclc::Record,
) -> anyhow::Result<()>
where
    S: signature::Keypair + spki::DynSignatureAlgorithmIdentifier,
    S::VerifyingKey: spki::EncodePublicKey,
{
    use x509_cert::ext::pkix::{KeyUsage, KeyUsages};

    // Subject Alternative Names
    if let sclc::Value::List(sans) = inputs.get("subjectAlternativeNames") {
        let mut general_names = vec![];
        for san_value in sans {
            let san = san_value.assert_str_ref()?;
            let general_name = parse_san(san)?;
            general_names.push(general_name);
        }
        if !general_names.is_empty() {
            let san_ext = x509_cert::ext::pkix::SubjectAltName(general_names);
            builder
                .add_extension(&san_ext)
                .map_err(|e| anyhow::anyhow!("failed to add SAN extension: {e}"))?;
        }
    }

    // Key Usage
    if let sclc::Value::List(usages) = inputs.get("keyUsage") {
        let mut ku: der::flagset::FlagSet<KeyUsages> = None.into();
        for usage_value in usages {
            let usage = usage_value.assert_str_ref()?;
            ku |= match usage {
                "digitalSignature" => KeyUsages::DigitalSignature,
                "nonRepudiation" | "contentCommitment" => KeyUsages::NonRepudiation,
                "keyEncipherment" => KeyUsages::KeyEncipherment,
                "dataEncipherment" => KeyUsages::DataEncipherment,
                "keyAgreement" => KeyUsages::KeyAgreement,
                "keyCertSign" => KeyUsages::KeyCertSign,
                "cRLSign" => KeyUsages::CRLSign,
                "encipherOnly" => KeyUsages::EncipherOnly,
                "decipherOnly" => KeyUsages::DecipherOnly,
                _ => anyhow::bail!("unsupported key usage: {usage}"),
            };
        }
        builder
            .add_extension(&KeyUsage(ku))
            .map_err(|e| anyhow::anyhow!("failed to add KeyUsage extension: {e}"))?;
    }

    // Extended Key Usage
    if let sclc::Value::List(usages) = inputs.get("extendedKeyUsage") {
        let mut eku_oids = vec![];
        for usage_value in usages {
            let usage = usage_value.assert_str_ref()?;
            let oid = match usage {
                "serverAuth" => const_oid::db::rfc5280::ID_KP_SERVER_AUTH,
                "clientAuth" => const_oid::db::rfc5280::ID_KP_CLIENT_AUTH,
                "codeSigning" => const_oid::db::rfc5280::ID_KP_CODE_SIGNING,
                "emailProtection" => const_oid::db::rfc5280::ID_KP_EMAIL_PROTECTION,
                "timeStamping" => const_oid::db::rfc5280::ID_KP_TIME_STAMPING,
                "ocspSigning" => const_oid::db::rfc5280::ID_KP_OCSP_SIGNING,
                _ => anyhow::bail!("unsupported extended key usage: {usage}"),
            };
            eku_oids.push(oid);
        }
        if !eku_oids.is_empty() {
            let eku = x509_cert::ext::pkix::ExtendedKeyUsage(eku_oids);
            builder
                .add_extension(&eku)
                .map_err(|e| anyhow::anyhow!("failed to add ExtendedKeyUsage extension: {e}"))?;
        }
    }

    Ok(())
}

fn parse_san(san: &str) -> anyhow::Result<x509_cert::ext::pkix::name::GeneralName> {
    use x509_cert::ext::pkix::name::GeneralName;

    if san.is_empty() {
        anyhow::bail!("SAN value must not be empty");
    }

    // Try parsing as IP address
    if let Ok(ip) = san.parse::<std::net::IpAddr>() {
        let bytes = match ip {
            std::net::IpAddr::V4(v4) => v4.octets().to_vec(),
            std::net::IpAddr::V6(v6) => v6.octets().to_vec(),
        };
        return Ok(GeneralName::IpAddress(der::asn1::OctetString::new(bytes)?));
    }

    // Check for email (contains @)
    if san.contains('@') {
        validate_email_san(san)?;
        return Ok(GeneralName::Rfc822Name(der::asn1::Ia5String::new(san)?));
    }

    // Default: DNS name
    validate_dns_san(san)?;
    Ok(GeneralName::DnsName(der::asn1::Ia5String::new(san)?))
}

/// Validate an email SAN has a minimal valid structure: local@domain with non-empty parts.
fn validate_email_san(email: &str) -> anyhow::Result<()> {
    let parts: Vec<&str> = email.splitn(2, '@').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        anyhow::bail!("invalid email SAN: expected local@domain, got {email:?}");
    }
    let domain = parts[1];
    if !domain.contains('.') {
        anyhow::bail!("invalid email SAN: domain must contain at least one dot, got {email:?}");
    }
    Ok(())
}

/// Validate a DNS SAN has valid hostname structure.
fn validate_dns_san(name: &str) -> anyhow::Result<()> {
    // Allow wildcard prefix
    let name = name.strip_prefix("*.").unwrap_or(name);
    if name.is_empty() {
        anyhow::bail!("invalid DNS SAN: name must not be empty");
    }
    for label in name.split('.') {
        if label.is_empty() {
            anyhow::bail!("invalid DNS SAN: labels must not be empty in {name:?}");
        }
        if label.len() > 63 {
            anyhow::bail!("invalid DNS SAN: label exceeds 63 characters in {name:?}");
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            anyhow::bail!("invalid DNS SAN: label contains invalid characters in {name:?}");
        }
        if label.starts_with('-') || label.ends_with('-') {
            anyhow::bail!("invalid DNS SAN: label must not start or end with a hyphen in {name:?}");
        }
    }
    Ok(())
}

#[async_trait::async_trait]
impl rtp::Plugin for CryptoPlugin {
    async fn create_resource(
        &mut self,
        _deployment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        self.dispatch(&id, inputs).await
    }

    async fn update_resource(
        &mut self,
        _deployment_qid: &str,
        id: ids::ResourceId,
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
