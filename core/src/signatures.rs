use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::Value;

// Release verification key. The signing key is never shipped with the app.
const TEMPLATE_PUBLIC_KEY: &str = "G8orRO/SOkC6vJ8RCUjfdMadsfQQEM3POD5K0c3d9lo=";

pub fn verify_template(doc: &Value, signature_base64: &str) -> Result<(), String> {
    let key = STANDARD
        .decode(TEMPLATE_PUBLIC_KEY)
        .map_err(|e| e.to_string())?;
    verify_with_key(doc, signature_base64, &key)
}

fn verify_with_key(doc: &Value, signature_base64: &str, key: &[u8]) -> Result<(), String> {
    crate::templates::validate_template_doc(doc)?;
    let key: [u8; 32] = key.try_into().map_err(|_| "invalid public key")?;
    let verifying_key = VerifyingKey::from_bytes(&key).map_err(|e| e.to_string())?;
    let bytes = serde_jcs::to_vec(doc).map_err(|e| e.to_string())?;
    let signature = STANDARD
        .decode(signature_base64)
        .map_err(|e| e.to_string())?;
    let signature = Signature::from_slice(&signature).map_err(|e| e.to_string())?;
    verifying_key
        .verify(&bytes, &signature)
        .map_err(|_| "template signature is invalid".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn verifies_canonical_document_and_rejects_mutation() {
        let key = SigningKey::from_bytes(&[17; 32]);
        let doc: Value =
            serde_json::from_str(include_str!("../builtin-templates/print-queue.v1.json")).unwrap();
        let sig = key.sign(&serde_jcs::to_vec(&doc).unwrap());
        let encoded = STANDARD.encode(sig.to_bytes());
        verify_with_key(&doc, &encoded, &key.verifying_key().to_bytes()).unwrap();
        let reordered: Value = serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
        verify_with_key(&reordered, &encoded, &key.verifying_key().to_bytes()).unwrap();
        let mut changed = doc.clone();
        changed["name"] = Value::String("Changed".into());
        assert!(verify_with_key(&changed, &encoded, &key.verifying_key().to_bytes()).is_err());
        assert!(verify_template(&doc, &encoded).is_err());
    }
}
