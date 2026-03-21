use clap::Subcommand;

use crate::client::SunbeamClient;
use crate::error::Result;
use crate::output::{self, OutputFormat};

// ---------------------------------------------------------------------------
// Top-level AuthCommand
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum AuthCommand {
    /// Identity management (Kratos).
    Identity {
        #[command(subcommand)]
        action: IdentityAction,
    },
    /// Session management (Kratos).
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Recovery codes and links (Kratos).
    Recovery {
        #[command(subcommand)]
        action: RecoveryAction,
    },
    /// Identity schemas (Kratos).
    Schema {
        #[command(subcommand)]
        action: SchemaAction,
    },
    /// Courier messages (Kratos).
    Courier {
        #[command(subcommand)]
        action: CourierAction,
    },
    /// Health check (Kratos).
    Health,
    /// OAuth2 client management (Hydra).
    Client {
        #[command(subcommand)]
        action: ClientAction,
    },
    /// JWK set management (Hydra).
    Jwk {
        #[command(subcommand)]
        action: JwkAction,
    },
    /// Trusted JWT issuer management (Hydra).
    Issuer {
        #[command(subcommand)]
        action: IssuerAction,
    },
    /// Token introspection and revocation (Hydra).
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
    /// Log in to both SSO and Gitea.
    Login {
        #[arg(long)]
        domain: Option<String>,
    },
    /// Log in to SSO only.
    Sso {
        #[arg(long)]
        domain: Option<String>,
    },
    /// Log in to Gitea only.
    Git {
        #[arg(long)]
        domain: Option<String>,
    },
    /// Log out.
    Logout,
    /// Show auth status.
    Status,
}

// ---------------------------------------------------------------------------
// Identity sub-commands
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum IdentityAction {
    /// List identities.
    List {
        #[arg(long)]
        page: Option<u32>,
        #[arg(long, default_value = "20")]
        page_size: Option<u32>,
    },
    /// Get an identity by ID.
    Get {
        #[arg(short, long)]
        id: String,
    },
    /// Create a new identity from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Update an identity (full replace) from JSON.
    Update {
        #[arg(short, long)]
        id: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete an identity.
    Delete {
        #[arg(short, long)]
        id: String,
    },
}

// ---------------------------------------------------------------------------
// Session sub-commands
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum SessionAction {
    /// List sessions.
    List {
        #[arg(long, default_value = "20")]
        page_size: Option<u32>,
        #[arg(long)]
        page_token: Option<String>,
        #[arg(long)]
        active: Option<bool>,
    },
    /// Get a session by ID.
    Get {
        #[arg(short, long)]
        id: String,
    },
    /// Extend a session.
    Extend {
        #[arg(short, long)]
        id: String,
    },
    /// Delete (disable) a session.
    Delete {
        #[arg(short, long)]
        id: String,
    },
}

// ---------------------------------------------------------------------------
// Recovery sub-commands
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum RecoveryAction {
    /// Create a recovery code for an identity.
    CreateCode {
        #[arg(short, long)]
        id: String,
        /// Duration string (e.g. "24h", "1h30m").
        #[arg(long)]
        expires_in: Option<String>,
    },
    /// Create a recovery link for an identity.
    CreateLink {
        #[arg(short, long)]
        id: String,
        /// Duration string (e.g. "24h", "1h30m").
        #[arg(long)]
        expires_in: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Schema sub-commands
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum SchemaAction {
    /// List identity schemas.
    List,
    /// Get a specific schema by ID.
    Get {
        #[arg(short, long)]
        id: String,
    },
}

// ---------------------------------------------------------------------------
// Courier sub-commands
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum CourierAction {
    /// List courier messages.
    List {
        #[arg(long, default_value = "20")]
        page_size: Option<u32>,
        #[arg(long)]
        page_token: Option<String>,
    },
    /// Get a courier message by ID.
    Get {
        #[arg(short, long)]
        id: String,
    },
}

// ---------------------------------------------------------------------------
// Client sub-commands (Hydra)
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum ClientAction {
    /// List OAuth2 clients.
    List {
        #[arg(long, default_value = "20")]
        limit: Option<u32>,
        #[arg(long)]
        offset: Option<u32>,
    },
    /// Get an OAuth2 client by ID.
    Get {
        #[arg(short, long)]
        id: String,
    },
    /// Create an OAuth2 client from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Update an OAuth2 client (full replace) from JSON.
    Update {
        #[arg(short, long)]
        id: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete an OAuth2 client.
    Delete {
        #[arg(short, long)]
        id: String,
    },
}

// ---------------------------------------------------------------------------
// JWK sub-commands (Hydra)
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum JwkAction {
    /// List keys in a JWK set.
    List {
        /// Name of the JWK set.
        #[arg(short = 'n', long)]
        set_name: String,
    },
    /// Get a specific key from a JWK set.
    Get {
        #[arg(short = 'n', long)]
        set_name: String,
        #[arg(short, long)]
        kid: String,
    },
    /// Create a new JWK set from JSON.
    Create {
        #[arg(short = 'n', long)]
        set_name: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete a JWK set.
    Delete {
        #[arg(short = 'n', long)]
        set_name: String,
    },
}

// ---------------------------------------------------------------------------
// Issuer sub-commands (Hydra)
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum IssuerAction {
    /// List trusted JWT issuers.
    List,
    /// Get a trusted JWT issuer by ID.
    Get {
        #[arg(short, long)]
        id: String,
    },
    /// Create a trusted JWT issuer from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete a trusted JWT issuer.
    Delete {
        #[arg(short, long)]
        id: String,
    },
}

// ---------------------------------------------------------------------------
// Token sub-commands (Hydra)
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum TokenAction {
    /// Introspect a token.
    Introspect {
        /// The token string to introspect.
        #[arg(short = 't', long)]
        token: String,
    },
    /// Delete all tokens for an OAuth2 client.
    Delete {
        /// The client_id whose tokens should be revoked.
        #[arg(long)]
        client_id: String,
    },
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn dispatch(
    cmd: AuthCommand,
    client: &SunbeamClient,
    output: OutputFormat,
) -> Result<()> {
    match cmd {
        // -- Kratos: Identity ---------------------------------------------------
        AuthCommand::Identity { action } => dispatch_identity(action, client, output).await,
        // -- Kratos: Session ----------------------------------------------------
        AuthCommand::Session { action } => dispatch_session(action, client, output).await,
        // -- Kratos: Recovery ---------------------------------------------------
        AuthCommand::Recovery { action } => dispatch_recovery(action, client, output).await,
        // -- Kratos: Schema -----------------------------------------------------
        AuthCommand::Schema { action } => dispatch_schema(action, client, output).await,
        // -- Kratos: Courier ----------------------------------------------------
        AuthCommand::Courier { action } => dispatch_courier(action, client, output).await,
        // -- Kratos: Health -----------------------------------------------------
        AuthCommand::Health => {
            let status = client.kratos().alive().await?;
            output::render(&status, output)
        }
        // -- Hydra: Client ------------------------------------------------------
        AuthCommand::Client { action } => dispatch_client(action, client, output).await,
        // -- Hydra: JWK ---------------------------------------------------------
        AuthCommand::Jwk { action } => dispatch_jwk(action, client, output).await,
        // -- Hydra: Issuer ------------------------------------------------------
        AuthCommand::Issuer { action } => dispatch_issuer(action, client, output).await,
        // -- Hydra: Token -------------------------------------------------------
        AuthCommand::Token { action } => dispatch_token(action, client, output).await,
        // -- SSO commands (delegate to existing auth module) --------------------
        AuthCommand::Login { domain } => {
            crate::auth::cmd_auth_login_all(domain.as_deref()).await
        }
        AuthCommand::Sso { domain } => {
            crate::auth::cmd_auth_sso_login(domain.as_deref()).await
        }
        AuthCommand::Git { domain } => {
            crate::auth::cmd_auth_git_login(domain.as_deref()).await
        }
        AuthCommand::Logout => crate::auth::cmd_auth_logout().await,
        AuthCommand::Status => crate::auth::cmd_auth_status().await,
    }
}

// ---------------------------------------------------------------------------
// Identity dispatch
// ---------------------------------------------------------------------------

async fn dispatch_identity(
    action: IdentityAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let kratos = client.kratos();
    match action {
        IdentityAction::List { page, page_size } => {
            let items = kratos.list_identities(page, page_size).await?;
            output::render_list(
                &items,
                &["ID", "SCHEMA", "STATE", "CREATED"],
                |i| {
                    vec![
                        i.id.clone(),
                        i.schema_id.clone(),
                        i.state.clone().unwrap_or_default(),
                        i.created_at.clone().unwrap_or_default(),
                    ]
                },
                fmt,
            )
        }
        IdentityAction::Get { id } => {
            let item = kratos.get_identity(&id).await?;
            output::render(&item, fmt)
        }
        IdentityAction::Create { data } => {
            let json = output::read_json_input(data.as_deref())?;
            let body: crate::identity::types::CreateIdentityBody =
                serde_json::from_value(json)?;
            let item = kratos.create_identity(&body).await?;
            output::render(&item, fmt)
        }
        IdentityAction::Update { id, data } => {
            let json = output::read_json_input(data.as_deref())?;
            let body: crate::identity::types::UpdateIdentityBody =
                serde_json::from_value(json)?;
            let item = kratos.update_identity(&id, &body).await?;
            output::render(&item, fmt)
        }
        IdentityAction::Delete { id } => {
            kratos.delete_identity(&id).await?;
            output::ok(&format!("Deleted identity {id}"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Session dispatch
// ---------------------------------------------------------------------------

async fn dispatch_session(
    action: SessionAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let kratos = client.kratos();
    match action {
        SessionAction::List {
            page_size,
            page_token,
            active,
        } => {
            let items = kratos
                .list_sessions(page_size, page_token.as_deref(), active)
                .await?;
            output::render_list(
                &items,
                &["ID", "ACTIVE", "EXPIRES", "AUTHENTICATED"],
                |s| {
                    vec![
                        s.id.clone(),
                        s.active.map_or("-".into(), |a| a.to_string()),
                        s.expires_at.clone().unwrap_or_default(),
                        s.authenticated_at.clone().unwrap_or_default(),
                    ]
                },
                fmt,
            )
        }
        SessionAction::Get { id } => {
            let item = kratos.get_session(&id).await?;
            output::render(&item, fmt)
        }
        SessionAction::Extend { id } => {
            let item = kratos.extend_session(&id).await?;
            output::render(&item, fmt)
        }
        SessionAction::Delete { id } => {
            kratos.disable_session(&id).await?;
            output::ok(&format!("Disabled session {id}"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Recovery dispatch
// ---------------------------------------------------------------------------

async fn dispatch_recovery(
    action: RecoveryAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let kratos = client.kratos();
    match action {
        RecoveryAction::CreateCode { id, expires_in } => {
            let item = kratos
                .create_recovery_code(&id, expires_in.as_deref())
                .await?;
            output::render(&item, fmt)
        }
        RecoveryAction::CreateLink { id, expires_in } => {
            let item = kratos
                .create_recovery_link(&id, expires_in.as_deref())
                .await?;
            output::render(&item, fmt)
        }
    }
}

// ---------------------------------------------------------------------------
// Schema dispatch
// ---------------------------------------------------------------------------

async fn dispatch_schema(
    action: SchemaAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let kratos = client.kratos();
    match action {
        SchemaAction::List => {
            let items = kratos.list_schemas().await?;
            output::render_list(
                &items,
                &["ID"],
                |s| vec![s.id.clone()],
                fmt,
            )
        }
        SchemaAction::Get { id } => {
            let item = kratos.get_schema(&id).await?;
            output::render(&item, fmt)
        }
    }
}

// ---------------------------------------------------------------------------
// Courier dispatch
// ---------------------------------------------------------------------------

async fn dispatch_courier(
    action: CourierAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let kratos = client.kratos();
    match action {
        CourierAction::List {
            page_size,
            page_token,
        } => {
            let items = kratos
                .list_courier_messages(page_size, page_token.as_deref())
                .await?;
            output::render_list(
                &items,
                &["ID", "STATUS", "TYPE", "RECIPIENT", "SUBJECT"],
                |m| {
                    vec![
                        m.id.clone(),
                        m.status.clone(),
                        m.r#type.clone(),
                        m.recipient.clone(),
                        m.subject.clone(),
                    ]
                },
                fmt,
            )
        }
        CourierAction::Get { id } => {
            let item = kratos.get_courier_message(&id).await?;
            output::render(&item, fmt)
        }
    }
}

// ---------------------------------------------------------------------------
// Client dispatch (Hydra)
// ---------------------------------------------------------------------------

async fn dispatch_client(
    action: ClientAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let hydra = client.hydra();
    match action {
        ClientAction::List { limit, offset } => {
            let items = hydra.list_clients(limit, offset).await?;
            output::render_list(
                &items,
                &["CLIENT_ID", "NAME", "SCOPE"],
                |c| {
                    vec![
                        c.client_id.clone().unwrap_or_default(),
                        c.client_name.clone().unwrap_or_default(),
                        c.scope.clone().unwrap_or_default(),
                    ]
                },
                fmt,
            )
        }
        ClientAction::Get { id } => {
            let item = hydra.get_client(&id).await?;
            output::render(&item, fmt)
        }
        ClientAction::Create { data } => {
            let json = output::read_json_input(data.as_deref())?;
            let body: crate::auth::hydra::types::OAuth2Client =
                serde_json::from_value(json)?;
            let item = hydra.create_client(&body).await?;
            output::render(&item, fmt)
        }
        ClientAction::Update { id, data } => {
            let json = output::read_json_input(data.as_deref())?;
            let body: crate::auth::hydra::types::OAuth2Client =
                serde_json::from_value(json)?;
            let item = hydra.update_client(&id, &body).await?;
            output::render(&item, fmt)
        }
        ClientAction::Delete { id } => {
            hydra.delete_client(&id).await?;
            output::ok(&format!("Deleted OAuth2 client {id}"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// JWK dispatch (Hydra)
// ---------------------------------------------------------------------------

async fn dispatch_jwk(
    action: JwkAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let hydra = client.hydra();
    match action {
        JwkAction::List { set_name } => {
            let item = hydra.get_jwk_set(&set_name).await?;
            output::render(&item, fmt)
        }
        JwkAction::Get { set_name, kid } => {
            let item = hydra.get_jwk_key(&set_name, &kid).await?;
            output::render(&item, fmt)
        }
        JwkAction::Create { set_name, data } => {
            let json = output::read_json_input(data.as_deref())?;
            let body: crate::auth::hydra::types::CreateJwkBody =
                serde_json::from_value(json)?;
            let item = hydra.create_jwk_set(&set_name, &body).await?;
            output::render(&item, fmt)
        }
        JwkAction::Delete { set_name } => {
            hydra.delete_jwk_set(&set_name).await?;
            output::ok(&format!("Deleted JWK set {set_name}"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Issuer dispatch (Hydra)
// ---------------------------------------------------------------------------

async fn dispatch_issuer(
    action: IssuerAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let hydra = client.hydra();
    match action {
        IssuerAction::List => {
            let items = hydra.list_trusted_issuers().await?;
            output::render_list(
                &items,
                &["ID", "ISSUER", "SUBJECT", "EXPIRES"],
                |i| {
                    vec![
                        i.id.clone().unwrap_or_default(),
                        i.issuer.clone(),
                        i.subject.clone(),
                        i.expires_at.clone().unwrap_or_default(),
                    ]
                },
                fmt,
            )
        }
        IssuerAction::Get { id } => {
            let item = hydra.get_trusted_issuer(&id).await?;
            output::render(&item, fmt)
        }
        IssuerAction::Create { data } => {
            let json = output::read_json_input(data.as_deref())?;
            let body: crate::auth::hydra::types::TrustedJwtIssuer =
                serde_json::from_value(json)?;
            let item = hydra.create_trusted_issuer(&body).await?;
            output::render(&item, fmt)
        }
        IssuerAction::Delete { id } => {
            hydra.delete_trusted_issuer(&id).await?;
            output::ok(&format!("Deleted trusted issuer {id}"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Token dispatch (Hydra)
// ---------------------------------------------------------------------------

async fn dispatch_token(
    action: TokenAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let hydra = client.hydra();
    match action {
        TokenAction::Introspect { token } => {
            let item = hydra.introspect_token(&token).await?;
            output::render(&item, fmt)
        }
        TokenAction::Delete { client_id } => {
            hydra.delete_tokens_for_client(&client_id).await?;
            output::ok(&format!("Deleted tokens for client {client_id}"));
            Ok(())
        }
    }
}
