//! LiveKit media service types.

use serde::{Deserialize, Deserializer, Serialize};

/// Deserialize a value that may be either a string or an integer as i64.
fn deserialize_string_or_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let v: Option<serde_json::Value> = Option::deserialize(d)?;
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => Ok(n.as_i64()),
        Some(serde_json::Value::String(s)) => Ok(s.parse().ok()),
        _ => Ok(None),
    }
}

/// A LiveKit room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Room {
    #[serde(default)]
    pub sid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub empty_timeout: Option<u32>,
    #[serde(default)]
    pub max_participants: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_string_or_i64")]
    pub creation_time: Option<i64>,
    #[serde(default)]
    pub metadata: Option<String>,
    #[serde(default)]
    pub num_participants: Option<u32>,
    #[serde(default)]
    pub num_publishers: Option<u32>,
}

/// Response from ListRooms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRoomsResponse {
    #[serde(default)]
    pub rooms: Vec<Room>,
}

/// Participant information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticipantInfo {
    #[serde(default)]
    pub sid: String,
    #[serde(default)]
    pub identity: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub state: Option<i32>,
    #[serde(default)]
    pub metadata: Option<String>,
    #[serde(default)]
    pub joined_at: Option<i64>,
    #[serde(default)]
    pub is_publisher: Option<bool>,
}

/// Response from ListParticipants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListParticipantsResponse {
    #[serde(default)]
    pub participants: Vec<ParticipantInfo>,
}

/// Response from MutePublishedTrack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MuteTrackResponse {
    #[serde(default)]
    pub track: Option<serde_json::Value>,
}

/// Egress information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EgressInfo {
    #[serde(default)]
    pub egress_id: String,
    #[serde(default)]
    pub room_id: Option<String>,
    #[serde(default)]
    pub room_name: Option<String>,
    #[serde(default)]
    pub status: Option<i32>,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub ended_at: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Response from ListEgress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListEgressResponse {
    #[serde(default)]
    pub items: Vec<EgressInfo>,
}

/// Video grant claims for access tokens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VideoGrants {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "roomCreate")]
    pub room_create: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "roomList")]
    pub room_list: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "roomJoin")]
    pub room_join: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "canPublish")]
    pub can_publish: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "canSubscribe")]
    pub can_subscribe: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "canPublishData")]
    pub can_publish_data: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "roomAdmin")]
    pub room_admin: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "roomRecord")]
    pub room_record: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_room_roundtrip() {
        let json = serde_json::json!({
            "sid": "RM_abc123",
            "name": "my-room",
            "max_participants": 50,
            "creation_time": 1700000000i64,
            "num_participants": 3
        });
        let room: Room = serde_json::from_value(json).unwrap();
        assert_eq!(room.sid, "RM_abc123");
        assert_eq!(room.name, "my-room");
        assert_eq!(room.max_participants, Some(50));
    }

    #[test]
    fn test_list_rooms_response() {
        let json = serde_json::json!({
            "rooms": [
                {"sid": "RM_1", "name": "room-1"},
                {"sid": "RM_2", "name": "room-2"}
            ]
        });
        let resp: ListRoomsResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.rooms.len(), 2);
    }

    #[test]
    fn test_participant_info() {
        let json = serde_json::json!({
            "sid": "PA_abc",
            "identity": "user@example.com",
            "name": "Alice",
            "is_publisher": true
        });
        let p: ParticipantInfo = serde_json::from_value(json).unwrap();
        assert_eq!(p.identity, "user@example.com");
        assert_eq!(p.is_publisher, Some(true));
    }

    #[test]
    fn test_egress_info() {
        let json = serde_json::json!({
            "egress_id": "EG_abc",
            "room_name": "my-room",
            "status": 1,
            "started_at": 1700000000i64
        });
        let e: EgressInfo = serde_json::from_value(json).unwrap();
        assert_eq!(e.egress_id, "EG_abc");
    }

    #[test]
    fn test_video_grants_serialization() {
        let grants = VideoGrants {
            room_create: Some(true),
            room_join: Some(true),
            room: Some("my-room".into()),
            can_publish: Some(true),
            can_subscribe: Some(true),
            ..Default::default()
        };
        let json = serde_json::to_value(&grants).unwrap();
        assert_eq!(json["roomCreate"], true);
        assert_eq!(json["roomJoin"], true);
        assert!(json.get("roomList").is_none());
        assert!(json.get("canPublishData").is_none());
    }

    #[test]
    fn test_empty_list_rooms() {
        let json = serde_json::json!({});
        let resp: ListRoomsResponse = serde_json::from_value(json).unwrap();
        assert!(resp.rooms.is_empty());
    }

    #[test]
    fn test_mute_track_response() {
        let json = serde_json::json!({"track": {"sid": "TR_abc"}});
        let r: MuteTrackResponse = serde_json::from_value(json).unwrap();
        assert!(r.track.is_some());
    }
}
