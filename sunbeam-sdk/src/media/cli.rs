use clap::Subcommand;

use crate::client::SunbeamClient;
use crate::error::Result;
use crate::media::types::VideoGrants;
use crate::media::LiveKitClient;
use crate::output::{self, OutputFormat};

// ---------------------------------------------------------------------------
// Top-level MediaCommand
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum MediaCommand {
    /// Room management.
    Room {
        #[command(subcommand)]
        action: RoomAction,
    },
    /// Participant management.
    Participant {
        #[command(subcommand)]
        action: ParticipantAction,
    },
    /// Egress management.
    Egress {
        #[command(subcommand)]
        action: EgressAction,
    },
    /// Token operations.
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
}

// ---------------------------------------------------------------------------
// Room sub-commands
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum RoomAction {
    /// List all rooms.
    List,
    /// Create a room.
    Create {
        /// Room name.
        #[arg(short, long)]
        name: String,
        /// Maximum number of participants.
        #[arg(long)]
        max_participants: Option<u32>,
    },
    /// Delete a room.
    Delete {
        /// Room name.
        #[arg(short, long)]
        name: String,
    },
}

// ---------------------------------------------------------------------------
// Participant sub-commands
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum ParticipantAction {
    /// List participants in a room.
    List {
        /// Room name.
        #[arg(short, long)]
        room: String,
    },
    /// Get a participant.
    Get {
        /// Room name.
        #[arg(short, long)]
        room: String,
        /// Participant identity.
        #[arg(short, long)]
        identity: String,
    },
    /// Remove a participant from a room.
    Remove {
        /// Room name.
        #[arg(short, long)]
        room: String,
        /// Participant identity.
        #[arg(short, long)]
        identity: String,
    },
}

// ---------------------------------------------------------------------------
// Egress sub-commands
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum EgressAction {
    /// List egress sessions for a room.
    List {
        /// Room name.
        #[arg(short, long)]
        room: String,
    },
    /// Start a room composite egress.
    StartRoomComposite {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Start a track egress.
    StartTrack {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Stop an egress session.
    Stop {
        /// Egress ID.
        #[arg(short, long)]
        egress_id: String,
    },
}

// ---------------------------------------------------------------------------
// Token sub-commands
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum TokenAction {
    /// Generate a LiveKit access token.
    Generate {
        /// LiveKit API key.
        #[arg(long)]
        api_key: String,
        /// LiveKit API secret.
        #[arg(long)]
        api_secret: String,
        /// Participant identity.
        #[arg(long)]
        identity: String,
        /// Room name to grant access to.
        #[arg(long)]
        room: Option<String>,
        /// Token TTL in seconds.
        #[arg(long, default_value = "3600")]
        ttl: u64,
    },
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn dispatch(
    cmd: MediaCommand,
    client: &SunbeamClient,
    output: OutputFormat,
) -> Result<()> {
    match cmd {
        MediaCommand::Room { action } => dispatch_room(action, client, output).await,
        MediaCommand::Participant { action } => {
            dispatch_participant(action, client, output).await
        }
        MediaCommand::Egress { action } => dispatch_egress(action, client, output).await,
        MediaCommand::Token { action } => dispatch_token(action, output),
    }
}

// ---------------------------------------------------------------------------
// Room dispatch
// ---------------------------------------------------------------------------

async fn dispatch_room(
    action: RoomAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let lk = client.livekit().await?;
    match action {
        RoomAction::List => {
            let resp = lk.list_rooms().await?;
            output::render_list(
                &resp.rooms,
                &["NAME", "SID", "PARTICIPANTS", "MAX", "CREATED"],
                |r| {
                    vec![
                        r.name.clone(),
                        r.sid.clone(),
                        r.num_participants
                            .map_or("-".into(), |n| n.to_string()),
                        r.max_participants
                            .map_or("-".into(), |n| n.to_string()),
                        r.creation_time
                            .map_or("-".into(), |t| t.to_string()),
                    ]
                },
                fmt,
            )
        }
        RoomAction::Create {
            name,
            max_participants,
        } => {
            let mut body = serde_json::json!({ "name": name });
            if let Some(max) = max_participants {
                body["max_participants"] = serde_json::json!(max);
            }
            let room = lk.create_room(&body).await?;
            output::render(&room, fmt)
        }
        RoomAction::Delete { name } => {
            lk.delete_room(&serde_json::json!({ "room": name })).await?;
            output::ok(&format!("Deleted room {name}"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Participant dispatch
// ---------------------------------------------------------------------------

async fn dispatch_participant(
    action: ParticipantAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let lk = client.livekit().await?;
    match action {
        ParticipantAction::List { room } => {
            let resp = lk
                .list_participants(&serde_json::json!({ "room": room }))
                .await?;
            output::render_list(
                &resp.participants,
                &["SID", "IDENTITY", "NAME", "STATE", "JOINED_AT"],
                |p| {
                    vec![
                        p.sid.clone(),
                        p.identity.clone(),
                        p.name.clone().unwrap_or_default(),
                        p.state.map_or("-".into(), |s| s.to_string()),
                        p.joined_at.map_or("-".into(), |t| t.to_string()),
                    ]
                },
                fmt,
            )
        }
        ParticipantAction::Get { room, identity } => {
            let info = lk
                .get_participant(&serde_json::json!({
                    "room": room,
                    "identity": identity,
                }))
                .await?;
            output::render(&info, fmt)
        }
        ParticipantAction::Remove { room, identity } => {
            lk.remove_participant(&serde_json::json!({
                "room": room,
                "identity": identity,
            }))
            .await?;
            output::ok(&format!("Removed participant {identity} from room {room}"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Egress dispatch
// ---------------------------------------------------------------------------

async fn dispatch_egress(
    action: EgressAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let lk = client.livekit().await?;
    match action {
        EgressAction::List { room } => {
            let resp = lk
                .list_egress(&serde_json::json!({ "room_name": room }))
                .await?;
            output::render_list(
                &resp.items,
                &["EGRESS_ID", "ROOM", "STATUS", "STARTED", "ENDED"],
                |e| {
                    vec![
                        e.egress_id.clone(),
                        e.room_name.clone().unwrap_or_default(),
                        e.status.map_or("-".into(), |s| s.to_string()),
                        e.started_at.map_or("-".into(), |t| t.to_string()),
                        e.ended_at.map_or("-".into(), |t| t.to_string()),
                    ]
                },
                fmt,
            )
        }
        EgressAction::StartRoomComposite { data } => {
            let body = output::read_json_input(data.as_deref())?;
            let info = lk.start_room_composite_egress(&body).await?;
            output::render(&info, fmt)
        }
        EgressAction::StartTrack { data } => {
            let body = output::read_json_input(data.as_deref())?;
            let info = lk.start_track_egress(&body).await?;
            output::render(&info, fmt)
        }
        EgressAction::Stop { egress_id } => {
            let info = lk
                .stop_egress(&serde_json::json!({ "egress_id": egress_id }))
                .await?;
            output::render(&info, fmt)
        }
    }
}

// ---------------------------------------------------------------------------
// Token dispatch
// ---------------------------------------------------------------------------

fn dispatch_token(action: TokenAction, fmt: OutputFormat) -> Result<()> {
    match action {
        TokenAction::Generate {
            api_key,
            api_secret,
            identity,
            room,
            ttl,
        } => {
            let grants = VideoGrants {
                room_join: Some(true),
                room,
                can_publish: Some(true),
                can_subscribe: Some(true),
                ..Default::default()
            };
            let token = LiveKitClient::generate_access_token(
                &api_key,
                &api_secret,
                &identity,
                &grants,
                ttl,
            )?;
            output::render(&serde_json::json!({ "token": token }), fmt)
        }
    }
}
