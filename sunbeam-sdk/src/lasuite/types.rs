//! Shared types for La Suite DRF-based services.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// DRF paginated response
// ---------------------------------------------------------------------------

/// Standard Django REST Framework paginated list response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DRFPage<T> {
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub previous: Option<String>,
    #[serde(default)]
    pub results: Vec<T>,
}

// ---------------------------------------------------------------------------
// People types
// ---------------------------------------------------------------------------

/// A contact in the People service.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Contact {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub organization: Option<String>,
    #[serde(default)]
    pub job_title: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A team in the People service.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Team {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub members: Option<Vec<String>>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A service provider in the People service.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceProvider {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A mail domain in the People service.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MailDomain {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Docs types
// ---------------------------------------------------------------------------

/// A document in the Docs service.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Document {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub is_public: Option<bool>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A document template.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Template {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub is_public: Option<bool>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A document version snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocVersion {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub document_id: Option<String>,
    #[serde(default)]
    pub version_number: Option<u64>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// An invitation to collaborate on a document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Invitation {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub document_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Meet types
// ---------------------------------------------------------------------------

/// A meeting room.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeetRoom {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub is_public: Option<bool>,
    #[serde(default)]
    pub configuration: Option<serde_json::Value>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A recording of a meeting.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Recording {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub room_id: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub created_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Drive types
// ---------------------------------------------------------------------------

/// A file in the Drive service.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DriveFile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A folder in the Drive service.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DriveFolder {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A file sharing record.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileShare {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub file_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// A file permission entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilePermission {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub file_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub can_read: Option<bool>,
    #[serde(default)]
    pub can_write: Option<bool>,
}

// ---------------------------------------------------------------------------
// Messages types
// ---------------------------------------------------------------------------

/// A mailbox.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Mailbox {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// An email message.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmailMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub from_address: Option<String>,
    #[serde(default)]
    pub to_addresses: Option<Vec<String>>,
    #[serde(default)]
    pub cc_addresses: Option<Vec<String>>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub is_read: Option<bool>,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// A mail folder.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MailFolder {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub message_count: Option<u64>,
    #[serde(default)]
    pub unread_count: Option<u64>,
}

/// A mail contact.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MailContact {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

// ---------------------------------------------------------------------------
// Calendars types
// ---------------------------------------------------------------------------

/// A calendar.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Calendar {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub is_default: Option<bool>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A calendar event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CalEvent {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub all_day: Option<bool>,
    #[serde(default)]
    pub attendees: Option<Vec<String>>,
    #[serde(default)]
    pub calendar_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Find types
// ---------------------------------------------------------------------------

/// A search result from the Find service.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchResult {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub created_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drf_page_deserialize() {
        let json = r#"{
            "count": 2,
            "next": "https://example.com/api/v1.0/contacts/?page=2",
            "previous": null,
            "results": [
                {"id": "1", "first_name": "Alice"},
                {"id": "2", "first_name": "Bob"}
            ]
        }"#;
        let page: DRFPage<Contact> = serde_json::from_str(json).unwrap();
        assert_eq!(page.count, 2);
        assert!(page.next.is_some());
        assert!(page.previous.is_none());
        assert_eq!(page.results.len(), 2);
        assert_eq!(page.results[0].first_name.as_deref(), Some("Alice"));
    }

    #[test]
    fn test_drf_page_empty() {
        let json = r#"{"count": 0, "next": null, "previous": null, "results": []}"#;
        let page: DRFPage<Contact> = serde_json::from_str(json).unwrap();
        assert_eq!(page.count, 0);
        assert!(page.results.is_empty());
    }

    #[test]
    fn test_drf_page_defaults() {
        let json = r#"{"results": []}"#;
        let page: DRFPage<Document> = serde_json::from_str(json).unwrap();
        assert_eq!(page.count, 0);
        assert!(page.next.is_none());
    }

    #[test]
    fn test_contact_roundtrip() {
        let c = Contact {
            id: "abc".into(),
            first_name: Some("Alice".into()),
            last_name: Some("Smith".into()),
            email: Some("alice@example.com".into()),
            phone: None,
            avatar: None,
            organization: None,
            job_title: None,
            notes: None,
            created_at: None,
            updated_at: None,
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: Contact = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.id, "abc");
        assert_eq!(c2.first_name.as_deref(), Some("Alice"));
    }

    #[test]
    fn test_document_defaults() {
        let json = r#"{"id": "doc-1"}"#;
        let d: Document = serde_json::from_str(json).unwrap();
        assert_eq!(d.id, "doc-1");
        assert!(d.title.is_none());
        assert!(d.content.is_none());
    }

    #[test]
    fn test_meet_room_deserialize() {
        let json = r#"{"id": "room-1", "name": "Standup", "slug": "standup"}"#;
        let r: MeetRoom = serde_json::from_str(json).unwrap();
        assert_eq!(r.id, "room-1");
        assert_eq!(r.name.as_deref(), Some("Standup"));
    }

    #[test]
    fn test_calendar_event_deserialize() {
        let json = r#"{"id": "ev-1", "title": "Lunch", "start": "2026-01-01T12:00:00Z", "all_day": false}"#;
        let e: CalEvent = serde_json::from_str(json).unwrap();
        assert_eq!(e.id, "ev-1");
        assert_eq!(e.all_day, Some(false));
    }

    #[test]
    fn test_search_result_deserialize() {
        let json = r#"{"id": "sr-1", "title": "Found it", "score": 0.95}"#;
        let sr: SearchResult = serde_json::from_str(json).unwrap();
        assert_eq!(sr.id, "sr-1");
        assert_eq!(sr.score, Some(0.95));
    }

    #[test]
    fn test_email_message_deserialize() {
        let json = r#"{"id": "msg-1", "subject": "Hello", "to_addresses": ["bob@example.com"]}"#;
        let m: EmailMessage = serde_json::from_str(json).unwrap();
        assert_eq!(m.id, "msg-1");
        assert_eq!(m.to_addresses.as_ref().unwrap().len(), 1);
    }
}
