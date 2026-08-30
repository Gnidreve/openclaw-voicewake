//! Geräteidentität für den Gateway-WebSocket-Connect-Handshake.
//!
//! Ein WebSocket-Client, der sich nur mit dem gemeinsamen `gateway.auth.token`
//! anmeldet, aber keine Geräteidentität mitschickt, bekommt vom Gateway zwar
//! eine erfolgreiche Verbindung, aber alle selbst angeforderten Scopes werden
//! stillschweigend auf eine leere Menge zurückgesetzt (verifiziert im
//! OpenClaw-Quellcode, `shouldClearUnboundScopesForMissingDeviceIdentity` in
//! `src/gateway/server/ws-connection/connect-policy.ts`). Ohne Scopes
//! schlagen `sessions.messages.subscribe`/`chat.send` mit `MISSING_SCOPE` fehl.
//!
//! Der hier implementierte Ed25519-Signaturvertrag (Payload-Format, Feld- und
//! Byte-Kodierung) ist deshalb kein eigener Entwurf, sondern 1:1 aus dem
//! tatsächlichen OpenClaw-Quellcode übernommen und gegen ihn geprüft:
//!   - `packages/gateway-client/src/device-auth.ts` (`buildDeviceAuthPayloadV3`,
//!     `normalizeDeviceMetadataForAuth`)
//!   - `src/infra/device-identity.ts` (`deriveDeviceIdFromPublicKey`:
//!     SHA-256 über die rohen 32 Public-Key-Bytes, hex-kodiert)
//!   - `src/infra/ed25519-signature.ts` (Base64url ohne Padding für Public
//!     Key und Signatur; Signatur ist eine reine Ed25519-Signatur ohne
//!     Prehash über die UTF-8-Bytes des Payload-Strings)
//!
//! Nichts davon darf geändert werden, ohne das Gegenstück im OpenClaw-Server
//! erneut zu prüfen - eine falsche Byte-Reihenfolge oder Kodierung liefert
//! keinen Laufzeitfehler, sondern nur eine vom Gateway abgelehnte Signatur.

use anyhow::{Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Fester, nicht konfigurierbarer Pfad - die Geräteidentität ist ein
/// Berechtigungsnachweis (nach einmaliger Kopplung erkennt das Gateway dieses
/// Gerät wieder), kein Feature, das pro Konfiguration verschieden sein sollte.
pub fn identity_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .context("HOME ist nicht gesetzt - kann keinen Pfad für die Geräteidentität bestimmen")?;
    Ok(PathBuf::from(home)
        .join(".openclaw-voicebridge")
        .join("device_identity.json"))
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredIdentity {
    /// Der rohe 32-Byte Ed25519-Seed, Base64url ohne Padding.
    private_key_seed_b64url: String,
}

/// Eine persistierte Ed25519-Geräteidentität.
pub struct DeviceIdentity {
    signing_key: SigningKey,
}

impl std::fmt::Debug for DeviceIdentity {
    /// Absichtlich ohne Schlüsselmaterial - nur der öffentliche, ohnehin über
    /// die Gateway-Verbindung übertragene Teil ist zum Debuggen nötig.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("device_id", &self.device_id())
            .finish_non_exhaustive()
    }
}

/// Eingaben für den v3-Signaturvertrag - Feld für Feld exakt wie
/// `buildDeviceAuthPayloadV3` in `packages/gateway-client/src/device-auth.ts`.
pub struct V3PayloadParams<'a> {
    pub device_id: &'a str,
    pub client_id: &'a str,
    pub client_mode: &'a str,
    pub role: &'a str,
    pub scopes: &'a [String],
    pub signed_at_ms: i64,
    pub token: Option<&'a str>,
    pub nonce: &'a str,
    pub platform: Option<&'a str>,
    pub device_family: Option<&'a str>,
}

/// Entspricht `normalizeDeviceMetadataForAuth`: nur ASCII-Großbuchstaben
/// werden kleingeschrieben (keine volle Unicode-Konvertierung), da die
/// Signatur beim Gateway byte-für-byte verglichen wird.
fn normalize_device_metadata_for_auth(value: Option<&str>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect()
}

/// Baut den zu signierenden v3-Payload-String. Reine Funktion, unabhängig
/// von der Geräteidentität testbar.
pub fn build_device_auth_payload_v3(params: &V3PayloadParams<'_>) -> String {
    let scopes = params.scopes.join(",");
    let token = params.token.unwrap_or("");
    let platform = normalize_device_metadata_for_auth(params.platform);
    let device_family = normalize_device_metadata_for_auth(params.device_family);
    [
        "v3",
        params.device_id,
        params.client_id,
        params.client_mode,
        params.role,
        &scopes,
        &params.signed_at_ms.to_string(),
        token,
        params.nonce,
        &platform,
        &device_family,
    ]
    .join("|")
}

impl DeviceIdentity {
    /// Lädt die Geräteidentität von `path` oder legt beim ersten Start eine
    /// neue an. Ein vorhandener, aber nicht lesbarer/beschädigter Datensatz
    /// wird NICHT stillschweigend durch eine neue Identität ersetzt - das
    /// würde die beim Gateway bereits genehmigte Kopplung verwerfen und eine
    /// erneute manuelle Genehmigung erzwingen, ohne dass es dafür einen
    /// erkennbaren Grund gäbe.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(raw) => Self::from_stored_json(&raw, path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::create_and_persist(path),
            Err(e) => Err(e)
                .with_context(|| format!("Kann Geräteidentität nicht lesen: {}", path.display())),
        }
    }

    fn from_stored_json(raw: &str, path: &Path) -> Result<Self> {
        let stored: StoredIdentity = serde_json::from_str(raw)
            .with_context(|| format!("Geräteidentität ist beschädigt: {}", path.display()))?;
        let seed_bytes = URL_SAFE_NO_PAD
            .decode(&stored.private_key_seed_b64url)
            .with_context(|| {
                format!(
                    "Geräteidentität enthält ungültiges Base64url: {}",
                    path.display()
                )
            })?;
        let seed: [u8; 32] = seed_bytes.try_into().map_err(|_| {
            anyhow::anyhow!(
                "Geräteidentität-Seed in {} hat nicht die erwarteten 32 Bytes",
                path.display()
            )
        })?;
        Ok(Self {
            signing_key: SigningKey::from_bytes(&seed),
        })
    }

    fn create_and_persist(path: &Path) -> Result<Self> {
        let signing_key = SigningKey::generate(&mut OsRng);
        let stored = StoredIdentity {
            private_key_seed_b64url: URL_SAFE_NO_PAD.encode(signing_key.to_bytes()),
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Kann Verzeichnis für die Geräteidentität nicht anlegen: {}",
                    parent.display()
                )
            })?;
        }
        let json = serde_json::to_string_pretty(&stored)
            .context("Kann Geräteidentität nicht serialisieren")?;
        std::fs::write(path, json)
            .with_context(|| format!("Kann Geräteidentität nicht schreiben: {}", path.display()))?;
        restrict_permissions(path)?;
        Ok(Self { signing_key })
    }

    /// Roher 32-Byte Ed25519-Public-Key, Base64url ohne Padding - genau die
    /// Form, die `connect.params.device.publicKey` erwartet.
    pub fn public_key_b64url(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.signing_key.verifying_key().to_bytes())
    }

    /// `deriveDeviceIdFromPublicKey`: SHA-256 über die rohen Public-Key-Bytes,
    /// hex-kodiert.
    pub fn device_id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.signing_key.verifying_key().to_bytes());
        hex::encode(hasher.finalize())
    }

    /// Signiert `payload` (UTF-8-Bytes, keine Prehash-Stufe) und gibt die
    /// Signatur Base64url-kodiert ohne Padding zurück - die Form, die
    /// `connect.params.device.signature` erwartet.
    pub fn sign_payload(&self, payload: &str) -> String {
        let signature = self.signing_key.sign(payload.as_bytes());
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("Kann Zugriffsrechte nicht einschränken: {}", path.display()))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_identity_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "openclaw-voicebridge-test-device-identity-{}.json",
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn normalize_metadata_lowercases_only_ascii_and_trims() {
        assert_eq!(normalize_device_metadata_for_auth(Some(" MacOS ")), "macos");
        assert_eq!(normalize_device_metadata_for_auth(Some("")), "");
        assert_eq!(normalize_device_metadata_for_auth(None), "");
    }

    /// Regression: Die Byte-Reihenfolge und die Trennzeichen müssen exakt der
    /// `buildDeviceAuthPayloadV3`-Implementierung in
    /// `packages/gateway-client/src/device-auth.ts` entsprechen - eine
    /// abweichende Payload-Form ergibt eine vom Gateway zurückgewiesene
    /// Signatur, keinen offensichtlichen Fehler.
    #[test]
    fn v3_payload_matches_the_openclaw_reference_format() {
        let scopes = vec!["operator.read".to_string(), "operator.write".to_string()];
        let payload = build_device_auth_payload_v3(&V3PayloadParams {
            device_id: "abc123",
            client_id: "openclaw-voicebridge",
            client_mode: "operator",
            role: "operator",
            scopes: &scopes,
            signed_at_ms: 1737264000000,
            token: Some("shared-secret"),
            nonce: "nonce-value",
            platform: Some("macOS"),
            device_family: Some("Mac"),
        });
        assert_eq!(
            payload,
            "v3|abc123|openclaw-voicebridge|operator|operator|operator.read,operator.write|1737264000000|shared-secret|nonce-value|macos|mac"
        );
    }

    #[test]
    fn v3_payload_uses_empty_string_for_missing_token_and_metadata() {
        let scopes: Vec<String> = vec![];
        let payload = build_device_auth_payload_v3(&V3PayloadParams {
            device_id: "abc123",
            client_id: "openclaw-voicebridge",
            client_mode: "operator",
            role: "operator",
            scopes: &scopes,
            signed_at_ms: 42,
            token: None,
            nonce: "n",
            platform: None,
            device_family: None,
        });
        assert_eq!(
            payload,
            "v3|abc123|openclaw-voicebridge|operator|operator||42||n||"
        );
    }

    #[test]
    fn device_id_is_the_sha256_hex_of_the_raw_public_key() {
        let identity = DeviceIdentity::create_and_persist(&temp_identity_path())
            .expect("Identität sollte erzeugbar sein");
        let raw_key = URL_SAFE_NO_PAD
            .decode(identity.public_key_b64url())
            .expect("Public Key sollte gültiges Base64url sein");
        let mut hasher = Sha256::new();
        hasher.update(&raw_key);
        assert_eq!(identity.device_id(), hex::encode(hasher.finalize()));
    }

    #[test]
    fn signature_round_trips_through_ed25519_verification() {
        let identity = DeviceIdentity::create_and_persist(&temp_identity_path())
            .expect("Identität sollte erzeugbar sein");
        let payload = "v3|abc|def|operator|operator||1||nonce||";
        let signature_b64url = identity.sign_payload(payload);

        use ed25519_dalek::{Verifier, VerifyingKey};
        let raw_key = URL_SAFE_NO_PAD
            .decode(identity.public_key_b64url())
            .unwrap();
        let verifying_key = VerifyingKey::from_bytes(&raw_key.try_into().unwrap()).unwrap();
        let signature_bytes = URL_SAFE_NO_PAD.decode(signature_b64url).unwrap();
        let signature = ed25519_dalek::Signature::from_bytes(&signature_bytes.try_into().unwrap());
        assert!(verifying_key.verify(payload.as_bytes(), &signature).is_ok());
    }

    #[test]
    fn load_or_create_persists_the_same_identity_across_calls() {
        let path = temp_identity_path();
        let first = DeviceIdentity::load_or_create(&path).expect("erster Aufruf sollte anlegen");
        let second = DeviceIdentity::load_or_create(&path).expect("zweiter Aufruf sollte laden");
        assert_eq!(first.device_id(), second.device_id());
        assert_eq!(first.public_key_b64url(), second.public_key_b64url());
        let _ = std::fs::remove_file(&path);
    }

    /// Regression: Eine beschädigte Identitätsdatei darf nicht stillschweigend
    /// durch eine neue Identität ersetzt werden - das würde eine beim Gateway
    /// bereits genehmigte Kopplung unbemerkt verwerfen.
    #[test]
    fn corrupted_identity_file_is_an_error_not_a_silent_regeneration() {
        let path = temp_identity_path();
        std::fs::write(&path, "not valid json").expect("Testdatei sollte schreibbar sein");
        let err = DeviceIdentity::load_or_create(&path)
            .expect_err("beschädigte Identitätsdatei muss abgelehnt werden");
        assert!(err.to_string().contains("beschädigt"), "{err}");
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn created_identity_file_is_only_readable_by_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_identity_path();
        let _identity = DeviceIdentity::load_or_create(&path).expect("sollte anlegen");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "Geräteidentität sollte nur für den Besitzer lesbar sein"
        );
        let _ = std::fs::remove_file(&path);
    }
}
