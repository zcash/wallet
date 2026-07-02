//! Test utilities for keystore operations.

use super::KeyStore;
use crate::{components::database::Database, error::Error};

/// Generates a test age identity and its corresponding recipient.
///
/// Returns a tuple of (identity, recipient_string) suitable for testing.
pub(crate) fn generate_test_identity() -> (age::x25519::Identity, String) {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public();
    (identity, recipient.to_string())
}

/// Creates a test KeyStore with a freshly generated identity.
///
/// The keystore will be unlocked (identities loaded) and have recipients
/// initialized in the database.
pub(crate) async fn test_keystore(db: Database) -> Result<KeyStore, Error> {
    let (identity, recipient_string) = generate_test_identity();

    let keystore = KeyStore::new_for_testing(db, vec![Box::new(identity)]);

    // Initialize recipients so encryption operations work
    keystore
        .initialize_recipients(vec![recipient_string])
        .await?;

    Ok(keystore)
}
