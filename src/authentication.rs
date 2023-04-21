use crate::telemetry::spawn_blocking_with_tracing;
use anyhow::Context;
use secrecy::{Secret, ExposeSecret};
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use sqlx::PgPool;

pub struct Credentials {
    pub username: String,
    pub password: Secret<String>,
}

#[tracing::instrument(name = "Validate credentials", skip(credentials, pool))]
pub async fn validate_credentials(
    credentials: Credentials,
    pool: &PgPool,
) -> Result<uuid::Uuid, AuthError> {
    let mut user_id = None;
    let mut expected_password_hash = Secret::new(
        "$argon2id$v=19$m=15000,t=2,p=1$\
        gZiv/M1gPc22ElAH/Jh1Hw$\
        CARF$!R#1lgc648ypr241rffcCCcqwefqfvh"
            .to_string()
    );

    if let Some((stored_user_id, stored_password_hash)) = 
        get_stored_credentials(&credentials.username, &pool)
            .await?
    {
        user_id = Some(stored_user_id);
        expected_password_hash = stored_password_hash;
    }
    
    // Spawn a new thread to run verify password hash since it is a resource
    // intensive operation that could block the executor/async thread.
    // Include tracing so that the logs are useful.
    spawn_blocking_with_tracing(move || {
        verify_password_hash(expected_password_hash, credentials.password)
    })
    .await
    .context("Failed to spawn a blocking task")??;

    // This is only set to `Some` if we found credentials in the store
    // So, even if the default password ends up matching (somehow)
    // with the provided password,
    // we never authenticate a non-existing user.
    // You can easily add a unit test for that precise scenario.
    user_id
        .ok_or_else(|| anyhow::anyhow!("Unknown username"))
        .map_err(AuthError::InvalidCredentials)
}

#[tracing::instrument(
    name = "Verify password hash"
    skip(expected_password_hash, password_candidate)
)]
fn verify_password_hash(
    expected_password_hash: Secret<String>,
    password_candidate: Secret<String>,
) -> Result<(), AuthError> {
    let expected_password_hash = PasswordHash::new(
        &expected_password_hash.expose_secret()
        )
        .context("Failed to parse hash in PHC string format")
        .map_err(AuthError::UnexpectedError)?;

    Argon2::default()
        .verify_password(
            password_candidate.expose_secret().as_bytes(),
            &expected_password_hash
            )
        .context("Invalid password")
        .map_err(AuthError::InvalidCredentials)
}

#[tracing::instrument(name = "Get stored credentials", skip(username, pool))]
async fn get_stored_credentials(
    username: &str,
    pool: &PgPool,
) -> Result<Option<(uuid::Uuid, Secret<String>)>, anyhow::Error> {
    let row = sqlx::query!(
        r#"
        SELECT user_id, password_hash
        FROM users
        WHERE username = $1
        "#,
        username,
        )
        .fetch_optional(pool)
        .await
        .context("Failed to perform a query to validate auth credentials")?
        .map(|row| (row.user_id, Secret::new(row.password_hash)));

    Ok(row)
}



#[derive(thiserror::Error, Debug)]
pub enum AuthError {
    #[error("Invalid credentials")]
    InvalidCredentials(#[source] anyhow::Error),
    #[error(transparent)]
    UnexpectedError(#[from] anyhow::Error),
}
