# Status and Incidents

Skyr continuously reports on the health of every deployment and resource. When something goes wrong long enough to be worth your attention, Skyr opens an **incident** — a durable record of what happened and when. Incidents close automatically once the system observes recovery, and Skyr emails every member of the owning organization on each open and close event.

## Health

Every deployment and every resource has a rolled-up health status, shown as a colored badge on the website:

- **Healthy** — no open incidents.
- **Degraded** — one or more open incidents, none of them a `Crash`.
- **Down** — at least one open `Crash` incident.

The deployment's health reflects only the deployment itself. If the deployment is `Healthy` but one of its resources is `Down`, you'll see that on the resource — health does not aggregate up the resource tree.

Alongside the badge, the website surfaces the underlying counters: the timestamp of the last status report, the number of open incidents, and the worst category among them.

## Incident categories

When Skyr reports a failure, it self-classifies it into one of five categories. The categories are *consequence-oriented* — they describe what the user-visible impact is, not where the failure came from.

| Category | Meaning |
|---|---|
| `BadConfiguration` | Skyr is refusing to roll out configuration it has determined to be invalid. The system is working as intended; the configuration isn't. |
| `CannotProgress` | The thing itself is stable, but a derived or dependent piece could not be applied. |
| `InconsistentState` | The configured state has drifted from reality and reconciliation can't close the gap. |
| `SystemError` | A failure in Skyr's own infrastructure (broker, database, plugin host). The user's configuration is not at fault. |
| `Crash` | The thing is not behaving as intended and there is user-visible downtime. |

Categories are immutable once an incident is open: if a deployment that already has an open `CannotProgress` incident starts emitting `Crash` reports, a *new* incident is opened in parallel. Incidents do not escalate or de-escalate within a single record.

When an incident closes, it stays closed. Recurrence creates a brand-new incident with a fresh ID rather than re-opening the old one. There is no manual close, ack, or snooze; if a category produces noisy open/close churn, the fix is to retune Skyr's threshold rules, not to suppress notifications.

## Notifications

Every incident open and every incident close triggers an email to every member of the organization that owns the entity. There are no per-user opt-outs and no per-org routing list in v1; if you're a member, you're on the list.

Emails carry the incident ID, category, owning entity, and the most recent error message. Skyr de-duplicates redeliveries internally, so even when the underlying message bus retries, you'll see exactly one email per `(incident, open|close)` event.

## Where to look

**Website.** Health badges appear on:

- The deployment list and deployment detail page.
- The resource list and resource detail page.

A per-organization **Incidents** view at `/<org>/~i` lists all incidents in the org, filterable by category and open-only. Click an incident for the detail view, including links back to the owning deployment or resource.

**CLI.** The `skyr` CLI surfaces deployments and resources today. Incident-aware CLI commands are not part of v1.

## Heartbeats and silent failure

Skyr's deployment and resource engines emit a status report on **every** iteration — not just failures. That means if a deployment is `Desired` but no report has arrived in over the expected window, Skyr can detect that the worker is stuck and open a `SystemError` incident on its behalf. The cadence Skyr expects depends on the entity's operational state (e.g., `DESIRED` deployments are expected to report more frequently than `LINGERING` ones).
