//! CLI dispatch for Matrix chat commands.

use crate::client::SunbeamClient;
use crate::error::Result;
use crate::output::{self, OutputFormat};
use clap::Subcommand;

// ---------------------------------------------------------------------------
// Command tree
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum ChatCommand {
    /// Room management.
    Room {
        #[command(subcommand)]
        action: RoomAction,
    },
    /// Send, list, get and redact messages.
    Message {
        #[command(subcommand)]
        action: MessageAction,
    },
    /// Room state events.
    State {
        #[command(subcommand)]
        action: StateAction,
    },
    /// User profile management.
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// Device management.
    Device {
        #[command(subcommand)]
        action: DeviceAction,
    },
    /// User directory search.
    User {
        #[command(subcommand)]
        action: UserAction,
    },
    /// Show authenticated user identity.
    Whoami,
    /// Synchronise client state with the server.
    Sync {
        /// Pagination token from a previous sync.
        #[arg(long)]
        since: Option<String>,
        /// Filter ID or inline JSON filter.
        #[arg(long)]
        filter: Option<String>,
        /// Request full state (all room state events).
        #[arg(long)]
        full_state: bool,
        /// Presence mode (offline, unavailable, online).
        #[arg(long)]
        set_presence: Option<String>,
        /// Long-poll timeout in milliseconds.
        #[arg(long)]
        timeout: Option<u64>,
    },
}

// -- Room -------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum RoomAction {
    /// Create a new room.
    Create {
        /// JSON body (or - for stdin).
        #[arg(short = 'd', long = "data")]
        data: Option<String>,
    },
    /// List public rooms.
    List {
        /// Maximum number of rooms to return.
        #[arg(long)]
        limit: Option<u32>,
        /// Pagination token.
        #[arg(long)]
        since: Option<String>,
    },
    /// Search public rooms.
    Search {
        /// Search query.
        #[arg(short = 'q', long)]
        query: String,
        /// Maximum results.
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Join a room.
    Join {
        /// Room ID or alias (e.g. !abc:example.com or #room:example.com).
        #[arg(long)]
        room_id: String,
    },
    /// Leave a room.
    Leave {
        /// Room ID.
        #[arg(long)]
        room_id: String,
    },
    /// Invite a user to a room.
    Invite {
        /// Room ID.
        #[arg(long)]
        room_id: String,
        /// User ID to invite (e.g. @alice:example.com).
        #[arg(long)]
        user_id: String,
        /// Reason for the invite.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Kick a user from a room.
    Kick {
        /// Room ID.
        #[arg(long)]
        room_id: String,
        /// User ID to kick.
        #[arg(long)]
        user_id: String,
        /// Reason.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Ban a user from a room.
    Ban {
        /// Room ID.
        #[arg(long)]
        room_id: String,
        /// User ID to ban.
        #[arg(long)]
        user_id: String,
        /// Reason.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Unban a user from a room.
    Unban {
        /// Room ID.
        #[arg(long)]
        room_id: String,
        /// User ID to unban.
        #[arg(long)]
        user_id: String,
        /// Reason.
        #[arg(long)]
        reason: Option<String>,
    },
}

// -- Message ----------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum MessageAction {
    /// Send a message to a room.
    Send {
        /// Room ID.
        #[arg(long)]
        room_id: String,
        /// Message text body. If omitted, reads JSON from --data or stdin.
        #[arg(long)]
        body: Option<String>,
        /// Event type (default: m.room.message).
        #[arg(long, default_value = "m.room.message")]
        event_type: String,
        /// Raw JSON body for the event content (or - for stdin).
        #[arg(short = 'd', long = "data")]
        data: Option<String>,
    },
    /// List messages in a room.
    List {
        /// Room ID.
        #[arg(long)]
        room_id: String,
        /// Pagination direction (b = backwards, f = forwards).
        #[arg(long, default_value = "b")]
        dir: String,
        /// Pagination token.
        #[arg(long)]
        from: Option<String>,
        /// Maximum messages to return.
        #[arg(long)]
        limit: Option<u32>,
        /// Event filter (JSON string).
        #[arg(long)]
        filter: Option<String>,
    },
    /// Get a single event.
    Get {
        /// Room ID.
        #[arg(long)]
        room_id: String,
        /// Event ID.
        #[arg(long)]
        event_id: String,
    },
    /// Redact an event.
    Redact {
        /// Room ID.
        #[arg(long)]
        room_id: String,
        /// Event ID to redact.
        #[arg(long)]
        event_id: String,
        /// Reason for redaction.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Search messages across rooms.
    Search {
        /// Search query.
        #[arg(short = 'q', long)]
        query: String,
    },
}

// -- State ------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum StateAction {
    /// List all state events in a room.
    List {
        /// Room ID.
        #[arg(long)]
        room_id: String,
    },
    /// Get a specific state event.
    Get {
        /// Room ID.
        #[arg(long)]
        room_id: String,
        /// Event type (e.g. m.room.name).
        #[arg(long)]
        event_type: String,
        /// State key (default: empty string).
        #[arg(long, default_value = "")]
        state_key: String,
    },
    /// Set a state event in a room.
    Set {
        /// Room ID.
        #[arg(long)]
        room_id: String,
        /// Event type (e.g. m.room.name).
        #[arg(long)]
        event_type: String,
        /// State key (default: empty string).
        #[arg(long, default_value = "")]
        state_key: String,
        /// JSON body (or - for stdin).
        #[arg(short = 'd', long = "data")]
        data: Option<String>,
    },
}

// -- Profile ----------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum ProfileAction {
    /// Get a user's profile.
    Get {
        /// User ID (e.g. @alice:example.com).
        #[arg(long)]
        user_id: String,
    },
    /// Set the display name.
    SetName {
        /// User ID.
        #[arg(long)]
        user_id: String,
        /// New display name.
        #[arg(long)]
        name: String,
    },
    /// Set the avatar URL.
    SetAvatar {
        /// User ID.
        #[arg(long)]
        user_id: String,
        /// Avatar MXC URI.
        #[arg(long)]
        url: String,
    },
}

// -- Device -----------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum DeviceAction {
    /// List all devices for the authenticated user.
    List,
    /// Get information about a specific device.
    Get {
        /// Device ID.
        #[arg(long)]
        device_id: String,
    },
    /// Delete a device.
    Delete {
        /// Device ID.
        #[arg(long)]
        device_id: String,
        /// Interactive auth JSON (or - for stdin).
        #[arg(short = 'd', long = "data")]
        data: Option<String>,
    },
}

// -- User -------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum UserAction {
    /// Search the user directory.
    Search {
        /// Search query.
        #[arg(short = 'q', long)]
        query: String,
        /// Maximum results.
        #[arg(long)]
        limit: Option<u32>,
    },
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch a parsed [`ChatCommand`] against the Matrix homeserver.
pub async fn dispatch(client: &SunbeamClient, format: OutputFormat, cmd: ChatCommand) -> Result<()> {
    let m = client.matrix().await?;

    match cmd {
        // -- Whoami ---------------------------------------------------------
        ChatCommand::Whoami => {
            let resp = m.whoami().await?;
            output::render(&resp, format)
        }

        // -- Sync -----------------------------------------------------------
        ChatCommand::Sync {
            since,
            filter,
            full_state,
            set_presence,
            timeout,
        } => {
            let params = super::types::SyncParams {
                since,
                filter,
                full_state: if full_state { Some(true) } else { None },
                set_presence,
                timeout,
            };
            let resp = m.sync(&params).await?;
            output::render(&resp, format)
        }

        // -- Room -----------------------------------------------------------
        ChatCommand::Room { action } => dispatch_room(&m, format, action).await,

        // -- Message --------------------------------------------------------
        ChatCommand::Message { action } => dispatch_message(&m, format, action).await,

        // -- State ----------------------------------------------------------
        ChatCommand::State { action } => dispatch_state(&m, format, action).await,

        // -- Profile --------------------------------------------------------
        ChatCommand::Profile { action } => dispatch_profile(&m, format, action).await,

        // -- Device ---------------------------------------------------------
        ChatCommand::Device { action } => dispatch_device(&m, format, action).await,

        // -- User -----------------------------------------------------------
        ChatCommand::User { action } => dispatch_user(&m, format, action).await,
    }
}

// ---------------------------------------------------------------------------
// Room
// ---------------------------------------------------------------------------

async fn dispatch_room(
    m: &super::MatrixClient,
    format: OutputFormat,
    action: RoomAction,
) -> Result<()> {
    match action {
        RoomAction::Create { data } => {
            let body: super::types::CreateRoomRequest =
                serde_json::from_value(output::read_json_input(data.as_deref())?)?;
            let resp = m.create_room(&body).await?;
            output::render(&resp, format)
        }

        RoomAction::List { limit, since } => {
            let resp = m.list_public_rooms(limit, since.as_deref()).await?;
            output::render_list(
                &resp.chunk,
                &["ROOM_ID", "NAME", "MEMBERS", "TOPIC"],
                |r| {
                    vec![
                        r.room_id.clone(),
                        r.name.clone().unwrap_or_default(),
                        r.num_joined_members.to_string(),
                        r.topic.clone().unwrap_or_default(),
                    ]
                },
                format,
            )
        }

        RoomAction::Search { query, limit } => {
            let body = super::types::SearchPublicRoomsRequest {
                limit,
                since: None,
                filter: Some(serde_json::json!({ "generic_search_term": query })),
                include_all_networks: None,
                third_party_instance_id: None,
            };
            let resp = m.search_public_rooms(&body).await?;
            output::render_list(
                &resp.chunk,
                &["ROOM_ID", "NAME", "MEMBERS", "TOPIC"],
                |r| {
                    vec![
                        r.room_id.clone(),
                        r.name.clone().unwrap_or_default(),
                        r.num_joined_members.to_string(),
                        r.topic.clone().unwrap_or_default(),
                    ]
                },
                format,
            )
        }

        RoomAction::Join { room_id } => {
            m.join_room_by_id(&room_id).await?;
            output::ok(&format!("Joined {room_id}"));
            Ok(())
        }

        RoomAction::Leave { room_id } => {
            m.leave_room(&room_id).await?;
            output::ok(&format!("Left {room_id}"));
            Ok(())
        }

        RoomAction::Invite {
            room_id,
            user_id,
            reason,
        } => {
            let body = super::types::InviteRequest { user_id: user_id.clone(), reason };
            m.invite(&room_id, &body).await?;
            output::ok(&format!("Invited {user_id} to {room_id}"));
            Ok(())
        }

        RoomAction::Kick {
            room_id,
            user_id,
            reason,
        } => {
            let body = super::types::KickRequest { user_id: user_id.clone(), reason };
            m.kick(&room_id, &body).await?;
            output::ok(&format!("Kicked {user_id} from {room_id}"));
            Ok(())
        }

        RoomAction::Ban {
            room_id,
            user_id,
            reason,
        } => {
            let body = super::types::BanRequest { user_id: user_id.clone(), reason };
            m.ban(&room_id, &body).await?;
            output::ok(&format!("Banned {user_id} from {room_id}"));
            Ok(())
        }

        RoomAction::Unban {
            room_id,
            user_id,
            reason,
        } => {
            let body = super::types::UnbanRequest { user_id: user_id.clone(), reason };
            m.unban(&room_id, &body).await?;
            output::ok(&format!("Unbanned {user_id} from {room_id}"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

async fn dispatch_message(
    m: &super::MatrixClient,
    format: OutputFormat,
    action: MessageAction,
) -> Result<()> {
    match action {
        MessageAction::Send {
            room_id,
            body,
            event_type,
            data,
        } => {
            let content: serde_json::Value = if let Some(text) = body {
                // Convenience: wrap plain text into m.room.message content.
                serde_json::json!({
                    "msgtype": "m.text",
                    "body": text,
                })
            } else {
                output::read_json_input(data.as_deref())?
            };

            let txn_id = format!(
                "cli-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );

            let resp = m.send_event(&room_id, &event_type, &txn_id, &content).await?;
            output::render(&resp, format)
        }

        MessageAction::List {
            room_id,
            dir,
            from,
            limit,
            filter,
        } => {
            let params = super::types::MessagesParams {
                dir,
                from,
                to: None,
                limit,
                filter,
            };
            let resp = m.get_messages(&room_id, &params).await?;
            output::render_list(
                &resp.chunk,
                &["EVENT_ID", "SENDER", "TYPE", "BODY"],
                |ev| {
                    vec![
                        ev.event_id.clone().unwrap_or_default(),
                        ev.sender.clone().unwrap_or_default(),
                        ev.event_type.clone(),
                        ev.content
                            .get("body")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    ]
                },
                format,
            )
        }

        MessageAction::Get { room_id, event_id } => {
            let ev = m.get_event(&room_id, &event_id).await?;
            output::render(&ev, format)
        }

        MessageAction::Redact {
            room_id,
            event_id,
            reason,
        } => {
            let txn_id = format!(
                "cli-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );
            let body = super::types::RedactRequest { reason };
            let resp = m.redact(&room_id, &event_id, &txn_id, &body).await?;
            output::render(&resp, format)
        }

        MessageAction::Search { query } => {
            let body = super::types::SearchRequest {
                search_categories: serde_json::json!({
                    "room_events": {
                        "search_term": query,
                    }
                }),
            };
            let resp = m.search_messages(&body).await?;
            output::render(&resp, format)
        }
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

async fn dispatch_state(
    m: &super::MatrixClient,
    format: OutputFormat,
    action: StateAction,
) -> Result<()> {
    match action {
        StateAction::List { room_id } => {
            let events = m.get_all_state(&room_id).await?;
            output::render_list(
                &events,
                &["TYPE", "STATE_KEY", "SENDER"],
                |ev| {
                    vec![
                        ev.event_type.clone(),
                        ev.state_key.clone(),
                        ev.sender.clone().unwrap_or_default(),
                    ]
                },
                format,
            )
        }

        StateAction::Get {
            room_id,
            event_type,
            state_key,
        } => {
            let val = m.get_state_event(&room_id, &event_type, &state_key).await?;
            output::render(&val, format)
        }

        StateAction::Set {
            room_id,
            event_type,
            state_key,
            data,
        } => {
            let body = output::read_json_input(data.as_deref())?;
            let resp = m
                .set_state_event(&room_id, &event_type, &state_key, &body)
                .await?;
            output::render(&resp, format)
        }
    }
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

async fn dispatch_profile(
    m: &super::MatrixClient,
    format: OutputFormat,
    action: ProfileAction,
) -> Result<()> {
    match action {
        ProfileAction::Get { user_id } => {
            let profile = m.get_profile(&user_id).await?;
            output::render(&profile, format)
        }

        ProfileAction::SetName { user_id, name } => {
            let body = super::types::SetDisplaynameRequest { displayname: name };
            m.set_displayname(&user_id, &body).await?;
            output::ok("Display name updated.");
            Ok(())
        }

        ProfileAction::SetAvatar { user_id, url } => {
            let body = super::types::SetAvatarUrlRequest { avatar_url: url };
            m.set_avatar_url(&user_id, &body).await?;
            output::ok("Avatar URL updated.");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Device
// ---------------------------------------------------------------------------

async fn dispatch_device(
    m: &super::MatrixClient,
    format: OutputFormat,
    action: DeviceAction,
) -> Result<()> {
    match action {
        DeviceAction::List => {
            let resp = m.list_devices().await?;
            output::render_list(
                &resp.devices,
                &["DEVICE_ID", "DISPLAY_NAME", "LAST_SEEN_IP"],
                |d| {
                    vec![
                        d.device_id.clone(),
                        d.display_name.clone().unwrap_or_default(),
                        d.last_seen_ip.clone().unwrap_or_default(),
                    ]
                },
                format,
            )
        }

        DeviceAction::Get { device_id } => {
            let device = m.get_device(&device_id).await?;
            output::render(&device, format)
        }

        DeviceAction::Delete { device_id, data } => {
            let body: super::types::DeleteDeviceRequest = if data.is_some() {
                serde_json::from_value(output::read_json_input(data.as_deref())?)?
            } else {
                super::types::DeleteDeviceRequest { auth: None }
            };
            m.delete_device(&device_id, &body).await?;
            output::ok(&format!("Device {device_id} deleted."));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// User
// ---------------------------------------------------------------------------

async fn dispatch_user(
    m: &super::MatrixClient,
    format: OutputFormat,
    action: UserAction,
) -> Result<()> {
    match action {
        UserAction::Search { query, limit } => {
            let body = super::types::UserSearchRequest {
                search_term: query,
                limit,
            };
            let resp = m.search_users(&body).await?;
            output::render_list(
                &resp.results,
                &["USER_ID", "DISPLAY_NAME"],
                |u| {
                    vec![
                        u.user_id.clone(),
                        u.display_name.clone().unwrap_or_default(),
                    ]
                },
                format,
            )
        }
    }
}
