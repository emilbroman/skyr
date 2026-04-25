//! Plain-text email rendering for notification requests.
//!
//! v1 keeps the templates small and free of HTML; richer templating is out of
//! scope. The subject summarises the event for inbox triage, and the body
//! lists the structured incident metadata followed by the most recent error
//! blurb (if any).

use nq::{NotificationEventType, NotificationRequest};

/// A rendered email, ready for an SMTP transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedEmail {
    pub subject: String,
    pub body: String,
}

/// Renders a [`NotificationRequest`] into a subject + plain-text body.
pub fn render(request: &NotificationRequest) -> RenderedEmail {
    let event_word = match request.event_type {
        NotificationEventType::Opened => "OPENED",
        NotificationEventType::Closed => "CLOSED",
    };

    let subject = format!(
        "[Skyr] Incident {event_word}: {category} on {entity}",
        category = request.category,
        entity = request.entity_qid,
    );

    let mut body = String::new();
    body.push_str(match request.event_type {
        NotificationEventType::Opened => "An incident has been opened.\n\n",
        NotificationEventType::Closed => "An incident has been closed.\n\n",
    });

    body.push_str(&format!("Incident:    {}\n", request.incident_id));
    body.push_str(&format!("Entity:      {}\n", request.entity_qid));
    body.push_str(&format!("Category:    {}\n", request.category));
    body.push_str(&format!(
        "Opened at:   {}\n",
        request.opened_at.to_rfc3339()
    ));

    if let Some(closed_at) = request.closed_at {
        body.push_str(&format!("Closed at:   {}\n", closed_at.to_rfc3339()));
    }

    if let Some(error) = request
        .last_error_message
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        body.push_str("\nLast error:\n");
        body.push_str(error);
        if !error.ends_with('\n') {
            body.push('\n');
        }
    }

    body.push_str("\n-- \nThis is an automated message from Skyr.\n");

    RenderedEmail { subject, body }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use nq::SeverityCategory;

    use super::*;

    fn open_request() -> NotificationRequest {
        NotificationRequest {
            incident_id: "01HZX9P5K2JN7YQVJ3Q6T4ZB8N".into(),
            event_type: NotificationEventType::Opened,
            entity_qid: "MyOrg/MyRepo::main@abc.def".into(),
            category: SeverityCategory::Crash,
            opened_at: "2026-04-25T12:00:00Z".parse::<DateTime<Utc>>().unwrap(),
            closed_at: None,
            last_error_message: Some("plugin returned EOF".into()),
        }
    }

    fn close_request() -> NotificationRequest {
        let mut req = open_request();
        req.event_type = NotificationEventType::Closed;
        req.closed_at = Some("2026-04-25T12:05:00Z".parse::<DateTime<Utc>>().unwrap());
        req.last_error_message = None;
        req
    }

    #[test]
    fn open_subject_mentions_open_and_category() {
        let r = render(&open_request());
        assert!(r.subject.contains("OPENED"));
        assert!(r.subject.contains("CRASH"));
        assert!(r.subject.contains("MyOrg/MyRepo::main@abc.def"));
    }

    #[test]
    fn close_subject_mentions_close() {
        let r = render(&close_request());
        assert!(r.subject.contains("CLOSED"));
    }

    #[test]
    fn open_body_includes_error_blurb() {
        let r = render(&open_request());
        assert!(r.body.contains("Last error:"));
        assert!(r.body.contains("plugin returned EOF"));
        assert!(!r.body.contains("Closed at:"));
    }

    #[test]
    fn close_body_includes_closed_at_and_omits_error_section() {
        let r = render(&close_request());
        assert!(r.body.contains("Closed at:"));
        assert!(!r.body.contains("Last error:"));
    }

    #[test]
    fn empty_error_message_does_not_render_section() {
        let mut req = open_request();
        req.last_error_message = Some(String::new());
        let r = render(&req);
        assert!(!r.body.contains("Last error:"));
    }
}
