//! Kafka security / SASL / TLS configuration.

/// Resolved security / SASL / TLS surface. Strings are post-`SecretExpr`
/// resolution (compose-layer responsibility).
#[derive(Debug, Clone, Default)]
pub struct KafkaSecurityConfig {
    /// `plaintext` (default), `ssl`, `sasl_plaintext`, `sasl_ssl`.
    pub security_protocol: Option<String>,
    /// SASL mechanism (e.g. `PLAIN`, `SCRAM-SHA-512`, `AWS_MSK_IAM`).
    pub sasl_mechanism: Option<String>,
    /// SASL username.
    pub sasl_username: Option<String>,
    /// SASL password.
    pub sasl_password: Option<String>,
    /// Kerberos service name.
    pub sasl_kerberos_service_name: Option<String>,
    /// Kerberos principal.
    pub sasl_kerberos_principal: Option<String>,
    /// Kerberos keytab path.
    pub sasl_kerberos_keytab: Option<String>,
    /// Kerberos custom kinit command.
    pub sasl_kerberos_kinit_cmd: Option<String>,
    /// Min seconds before kinit re-login.
    pub sasl_kerberos_min_time_before_relogin_secs: Option<u64>,
    /// OAUTHBEARER method.
    pub sasl_oauthbearer_method: Option<String>,
    /// OAUTHBEARER static config.
    pub sasl_oauthbearer_config: Option<String>,
    /// OAUTHBEARER OIDC client id.
    pub sasl_oauthbearer_client_id: Option<String>,
    /// OAUTHBEARER OIDC client secret.
    pub sasl_oauthbearer_client_secret: Option<String>,
    /// OAUTHBEARER OIDC token endpoint URL.
    pub sasl_oauthbearer_token_endpoint_url: Option<String>,
    /// OAUTHBEARER OIDC scope.
    pub sasl_oauthbearer_scope: Option<String>,
    /// OAUTHBEARER extensions.
    pub sasl_oauthbearer_extensions: Option<String>,
    /// Allow unsigned JWTs (dev-only).
    pub enable_sasl_oauthbearer_unsecure_jwt: Option<bool>,
    /// SSL CA file path.
    pub ssl_ca_location: Option<String>,
    /// SSL client certificate path.
    pub ssl_certificate_location: Option<String>,
    /// SSL client key path.
    pub ssl_key_location: Option<String>,
    /// SSL client key password.
    pub ssl_key_password: Option<String>,
    /// Inline PEM CA bundle.
    pub ssl_ca_pem: Option<String>,
    /// Inline PEM client cert.
    pub ssl_certificate_pem: Option<String>,
    /// Inline PEM client key.
    pub ssl_key_pem: Option<String>,
    /// PKCS12 keystore path.
    pub ssl_keystore_location: Option<String>,
    /// PKCS12 keystore password.
    pub ssl_keystore_password: Option<String>,
    /// SSL CRL path.
    pub ssl_crl_location: Option<String>,
    /// Cipher suites.
    pub ssl_cipher_suites: Option<String>,
    /// Allowed elliptic curves.
    pub ssl_curves_list: Option<String>,
    /// Allowed signature algorithms.
    pub ssl_sigalgs_list: Option<String>,
    /// Endpoint identification algorithm (`none` | `https`).
    pub ssl_endpoint_identification_algorithm: Option<String>,
    /// Verify peer certificate (default true).
    pub enable_ssl_certificate_verification: Option<bool>,
    /// HSM engine id.
    pub ssl_engine_id: Option<String>,
    /// HSM engine path.
    pub ssl_engine_location: Option<String>,
}
