// ═══════════════════════════════════════════════════════════
// Pre-scripted event scenarios
// ═══════════════════════════════════════════════════════════

export interface Scenario {
  id: string;
  title: string;
  icon: string;
  text: string;
}

export const SCENARIOS: Scenario[] = [
  {
    id: "client-escalation",
    title: "Client Escalation",
    icon: "\u{1F4E7}",
    text: `INCOMING EMAIL from client@acme.com
Subject: Q4 Deliverables Late
Body: We expected the Q4 deliverables last Friday. This is now 5 business days overdue and impacting our product launch timeline. We need an immediate status update and a revised delivery date. If we don't hear back by end of day, we'll need to escalate to your executive team.
— Jim, VP Operations, Acme Corp`,
  },
  {
    id: "server-alert",
    title: "Server Room Alert",
    icon: "\u{1F321}\uFE0F",
    text: `ALERT from monitoring system:
Server room B temperature reading: 85\u00B0F (threshold: 75\u00B0F)
Trend: rising 2\u00B0F/hour for the past 3 hours
Affected systems: primary application servers, database cluster
Risk: hardware damage if temperature exceeds 90\u00B0F within next 2.5 hours`,
  },
  {
    id: "expense-report",
    title: "Expense Report",
    icon: "\u{1F4B0}",
    text: `INVOICE RECEIVED:
Vendor: CloudCorp Inc
Invoice #: CC-4421
Amount: $4,200.00
Service: Cloud infrastructure — Q1 compute and storage
Payment terms: Net 30 (due in 22 days)
Vendor contact: billing@cloudcorp.com
Note: This is 15% higher than last quarter's invoice ($3,650)`,
  },
  {
    id: "calendar-conflict",
    title: "Calendar Conflict",
    icon: "\u{1F4C5}",
    text: `CALENDAR ALERT:
Sam Torres has 2 conflicting meetings at 2:00 PM today:
1) Board Strategy Review — organized by CEO, attendees: all C-suite, 90 min, conf room A
2) Engineering Sprint Planning — organized by Sam, attendees: 6 engineers, 60 min, conf room B
Both meetings were confirmed. Sprint planning has been on the calendar for 2 weeks. Board review was added yesterday.`,
  },
  {
    id: "new-hire",
    title: "New Hire Onboarding",
    icon: "\u{1F464}",
    text: `HR NOTIFICATION — New Employee Starting Monday:
Name: Casey Rivera
Role: Senior Developer
Team: Engineering (reports to Alex Chen)
Start date: Monday
Requirements: laptop (MacBook Pro 16"), badge access (building A, floors 2-4), desk assignment (engineering area preferred), onboarding packet, team introduction email, IT account provisioning (email, Slack, GitHub, cloud console)`,
  },
  {
    id: "security-incident",
    title: "Security Breach",
    icon: "\u{1F6A8}",
    text: `SECURITY ALERT from IT monitoring:
Unusual login activity detected on the admin console. 47 failed login attempts from IP 203.0.113.42 (geo: unknown) in the past 15 minutes, followed by 1 successful login using service account "deploy-bot".
No deployments are scheduled today. The account has read/write access to production infrastructure.
Recommend immediate investigation and potential credential rotation.`,
  },
];
